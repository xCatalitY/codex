use super::*;
use crate::agent::WorkflowAgentWorktree;
use crate::agent::control::SpawnAgentForkMode;
use crate::agent::control::SpawnAgentOptions;
use crate::agent::exceeds_thread_spawn_depth_limit;
use crate::agent::next_thread_spawn_depth;
use crate::agent::role::DEFAULT_ROLE_NAME;
use crate::agent::role::apply_role_to_config;
use crate::session::turn_context::TurnContext;
use crate::tools::code_mode::workflow_agent_transcript_target_for_cell;
use crate::tools::context::ToolCallSource;
use crate::tools::handlers::multi_agents_spec::SpawnAgentToolOptions;
use crate::tools::handlers::multi_agents_spec::create_spawn_agent_tool_v2;
use crate::turn_timing::now_unix_timestamp_ms;
use codex_git_utils::get_git_repo_root;
use codex_protocol::AgentPath;
use codex_protocol::protocol::Op;
use codex_protocol::protocol::TurnEnvironmentSelection;
use codex_tools::ToolSpec;
use codex_utils_absolute_path::AbsolutePathBuf;
use tokio::process::Command;

#[derive(Default)]
pub(crate) struct Handler {
    options: SpawnAgentToolOptions,
}

impl Handler {
    pub(crate) fn new(options: SpawnAgentToolOptions) -> Self {
        Self { options }
    }
}

impl ToolExecutor<ToolInvocation> for Handler {
    fn tool_name(&self) -> ToolName {
        ToolName::plain("spawn_agent")
    }

    fn spec(&self) -> ToolSpec {
        create_spawn_agent_tool_v2(self.options.clone())
    }

    fn handle(&self, invocation: ToolInvocation) -> codex_tools::ToolExecutorFuture<'_> {
        Box::pin(async move { handle_spawn_agent(invocation).await.map(boxed_tool_output) })
    }
}

async fn handle_spawn_agent(
    invocation: ToolInvocation,
) -> Result<SpawnAgentResult, FunctionCallError> {
    let ToolInvocation {
        session,
        turn,
        payload,
        call_id,
        source,
        ..
    } = invocation;
    let arguments = function_arguments(payload)?;
    let args: SpawnAgentArgs = parse_arguments(&arguments)?;
    if let Some(schema) = args.schema.as_ref()
        && !schema.is_object()
    {
        return Err(FunctionCallError::RespondToModel(
            "schema must be a JSON object final-output schema".to_string(),
        ));
    }
    let isolation = args
        .isolation
        .as_deref()
        .map(str::trim)
        .filter(|isolation| !isolation.is_empty());
    if let Some(isolation) = isolation {
        match isolation {
            "remote" => {
                return Err(FunctionCallError::RespondToModel(
                    "agent({isolation:'remote'}) is not available in this build".to_string(),
                ));
            }
            "worktree" => {}
            _ => {
                return Err(FunctionCallError::RespondToModel(
                    "isolation must be `worktree` or `remote`".to_string(),
                ));
            }
        };
    }
    let fork_mode = args.fork_mode()?;
    let role_name = args
        .agent_type
        .as_deref()
        .map(str::trim)
        .filter(|role| !role.is_empty());

    let mut message = args.message.clone();
    let final_output_json_schema = args.schema.clone();
    let session_source = turn.session_source.clone();
    let child_depth = next_thread_spawn_depth(&session_source);
    let max_depth = turn.config.agent_max_depth;
    if exceeds_thread_spawn_depth_limit(child_depth, max_depth) {
        return Err(FunctionCallError::RespondToModel(
            "Agent depth limit reached. Solve the task yourself.".to_string(),
        ));
    }
    let mut config =
        build_agent_spawn_config(&session.get_base_instructions().await, turn.as_ref())?;
    let mut environment_selections = turn.environments.to_selections();
    let workflow_worktree = if isolation == Some("worktree") {
        let worktree = create_workflow_agent_worktree(
            turn.as_ref(),
            args.task_name.as_str(),
            call_id.as_str(),
        )
        .await?;
        message = append_worktree_isolation_notice(
            &message,
            &worktree.path,
            workflow_turn_cwd(turn.as_ref()),
        );
        Some(worktree)
    } else {
        None
    };
    let initial_operation = parse_collab_input(Some(message.clone()), /*items*/ None)?;
    if let Some(service_tier) = args.service_tier.as_ref() {
        config.service_tier = Some(service_tier.clone());
    }
    if matches!(fork_mode, Some(SpawnAgentForkMode::FullHistory)) {
        reject_full_fork_spawn_overrides(
            role_name,
            args.model.as_deref(),
            args.reasoning_effort.clone(),
        )?;
    } else {
        apply_requested_spawn_agent_model_overrides(
            &session,
            turn.as_ref(),
            &mut config,
            args.model.as_deref(),
            args.reasoning_effort.clone(),
        )
        .await?;
        apply_role_to_config(&mut config, role_name)
            .await
            .map_err(FunctionCallError::RespondToModel)?;
    }
    apply_spawn_agent_service_tier(
        &session,
        &mut config,
        turn.config.service_tier.as_deref(),
        args.service_tier.as_deref(),
    )
    .await?;
    apply_spawn_agent_runtime_overrides(&mut config, turn.as_ref())?;
    if let Some(worktree) = workflow_worktree.as_ref() {
        retarget_spawn_to_worktree(&mut config, &mut environment_selections, &worktree.path);
    }
    let workflow_worktree_path = workflow_worktree
        .as_ref()
        .map(|worktree| worktree.path.display().to_string());

    let spawn_source = thread_spawn_source(
        session.thread_id,
        &turn.session_source,
        child_depth,
        role_name,
        Some(args.task_name.clone()),
    )?;
    let new_agent_path = spawn_source.get_agent_path().ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "spawned agent is missing a canonical task name".to_string(),
        )
    })?;
    let workflow_transcript_target =
        workflow_agent_transcript_target_for_spawn(turn.as_ref(), &source, &new_agent_path).await;
    let workflow_transcript_path = workflow_transcript_target
        .as_ref()
        .map(|target| target.transcript_path.clone());
    let workflow_transcript_mirror_path = workflow_transcript_target
        .as_ref()
        .and_then(|target| target.mirror_transcript_path.clone());
    let workflow_live_transcript = workflow_transcript_target.is_some();
    let workflow_tool_use_id = workflow_live_transcript.then(|| call_id.clone());
    let spawned_agent = Box::pin(
        session.services.agent_control.spawn_agent_with_metadata(
            config,
            match initial_operation {
                Op::UserInput { items, .. }
                    if items
                        .iter()
                        .all(|item| matches!(item, UserInput::Text { .. })) =>
                {
                    let author = turn
                        .session_source
                        .get_agent_path()
                        .unwrap_or_else(AgentPath::root);
                    let communication =
                        communication_from_tool_message(author, new_agent_path.clone(), message)
                            .with_final_output_json_schema(final_output_json_schema.clone());
                    Op::InterAgentCommunication { communication }
                }
                Op::UserInput {
                    items,
                    responsesapi_client_metadata,
                    additional_context,
                    thread_settings,
                    ..
                } => Op::UserInput {
                    items,
                    final_output_json_schema: final_output_json_schema.clone(),
                    responsesapi_client_metadata,
                    additional_context,
                    thread_settings,
                },
                initial_operation => initial_operation,
            },
            Some(spawn_source),
            SpawnAgentOptions {
                fork_parent_spawn_call_id: fork_mode.as_ref().map(|_| call_id.clone()),
                fork_mode,
                parent_thread_id: Some(session.thread_id),
                environments: Some(environment_selections),
                workflow_worktree,
                workflow_transcript_path,
                workflow_transcript_mirror_path,
            },
        ),
    )
    .await
    .map_err(collab_spawn_error)?;
    let new_thread_id = spawned_agent.thread_id;
    let agent_snapshot = session
        .services
        .agent_control
        .get_agent_config_snapshot(new_thread_id)
        .await;
    let nickname = agent_snapshot
        .as_ref()
        .and_then(|snapshot| snapshot.session_source.get_nickname())
        .or(spawned_agent.metadata.agent_nickname);
    session
        .send_event(
            &turn,
            SubAgentActivityEvent {
                event_id: call_id,
                occurred_at_ms: now_unix_timestamp_ms(),
                agent_thread_id: new_thread_id,
                agent_path: new_agent_path.clone(),
                kind: SubAgentActivityKind::Started,
            }
            .into(),
        )
        .await;
    let role_tag = role_name.unwrap_or(DEFAULT_ROLE_NAME);
    turn.session_telemetry.counter(
        "codex.multi_agent.spawn",
        /*inc*/ 1,
        &[("role", role_tag), ("version", "v2")],
    );
    let task_name = String::from(new_agent_path);

    let hide_agent_metadata = turn.config.multi_agent_v2.hide_spawn_agent_metadata;
    if hide_agent_metadata {
        Ok(SpawnAgentResult::HiddenMetadata {
            task_name,
            workflow_live_transcript,
            tool_use_id: workflow_tool_use_id,
            worktree_path: workflow_worktree_path,
        })
    } else {
        Ok(SpawnAgentResult::WithNickname {
            task_name,
            nickname,
            workflow_live_transcript,
            tool_use_id: workflow_tool_use_id,
            worktree_path: workflow_worktree_path,
        })
    }
}

async fn workflow_agent_transcript_target_for_spawn(
    turn: &TurnContext,
    source: &ToolCallSource,
    agent_path: &AgentPath,
) -> Option<crate::tools::code_mode::WorkflowAgentTranscriptTarget> {
    let ToolCallSource::CodeMode { cell_id, .. } = source else {
        return None;
    };
    workflow_agent_transcript_target_for_cell(turn, cell_id, agent_path.as_str()).await
}

async fn create_workflow_agent_worktree(
    turn: &TurnContext,
    task_name: &str,
    call_id: &str,
) -> Result<WorkflowAgentWorktree, FunctionCallError> {
    let repo_root = get_git_repo_root(workflow_turn_cwd(turn).as_path()).ok_or_else(|| {
        FunctionCallError::RespondToModel(
            "agent({isolation:'worktree'}) requires the current workflow cwd to be inside a local git repository"
                .to_string(),
        )
    })?;
    let repo_root = AbsolutePathBuf::from_absolute_path(&repo_root).map_err(|err| {
        FunctionCallError::RespondToModel(format!(
            "failed to resolve workflow git root for worktree isolation: {err}"
        ))
    })?;

    let worktree_parent = turn.config.codex_home.join("workflow-worktrees");
    tokio::fs::create_dir_all(worktree_parent.as_path())
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to create workflow worktree directory {}: {err}",
                worktree_parent.display()
            ))
        })?;
    let worktree_path = unique_worktree_path(&worktree_parent, task_name, call_id).await?;
    let output = Command::new("git")
        .arg("-C")
        .arg(repo_root.as_path())
        .arg("worktree")
        .arg("add")
        .arg("--detach")
        .arg(worktree_path.as_path())
        .arg("HEAD")
        .output()
        .await
        .map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to run git worktree add for workflow agent: {err}"
            ))
        })?;
    if !output.status.success() {
        return Err(FunctionCallError::RespondToModel(format!(
            "failed to create workflow agent worktree: {}",
            command_output_summary(&output.stderr)
        )));
    }

    Ok(WorkflowAgentWorktree {
        path: worktree_path,
        repo_root,
    })
}

fn workflow_turn_cwd(turn: &TurnContext) -> &AbsolutePathBuf {
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

async fn unique_worktree_path(
    parent: &AbsolutePathBuf,
    task_name: &str,
    call_id: &str,
) -> Result<AbsolutePathBuf, FunctionCallError> {
    let base = format!(
        "{}-{}",
        sanitize_worktree_component(task_name),
        sanitize_worktree_component(call_id)
    );
    for attempt in 0..100 {
        let name = if attempt == 0 {
            base.clone()
        } else {
            format!("{base}-{attempt}")
        };
        let path = parent.join(name);
        if !path.as_path().try_exists().map_err(|err| {
            FunctionCallError::RespondToModel(format!(
                "failed to check workflow worktree path {}: {err}",
                path.display()
            ))
        })? {
            return Ok(path);
        }
    }
    Err(FunctionCallError::RespondToModel(
        "failed to allocate a unique workflow worktree path".to_string(),
    ))
}

fn sanitize_worktree_component(value: &str) -> String {
    let mut out = String::new();
    for ch in value.chars() {
        if ch.is_ascii_alphanumeric() {
            out.push(ch.to_ascii_lowercase());
        } else if matches!(ch, '-' | '_' | '/' | ':' | '.') {
            out.push('-');
        }
        if out.len() >= 48 {
            break;
        }
    }
    let out = out.trim_matches('-').to_string();
    if out.is_empty() {
        "agent".to_string()
    } else {
        out
    }
}

fn command_output_summary(stderr: &[u8]) -> String {
    let text = String::from_utf8_lossy(stderr).trim().to_string();
    if text.is_empty() {
        "git exited without stderr".to_string()
    } else {
        text
    }
}

fn retarget_spawn_to_worktree(
    config: &mut crate::config::Config,
    environment_selections: &mut [TurnEnvironmentSelection],
    worktree_path: &AbsolutePathBuf,
) {
    config.cwd = worktree_path.clone();
    if !config
        .workspace_roots
        .iter()
        .any(|root| root == worktree_path)
    {
        config.workspace_roots.insert(0, worktree_path.clone());
    }
    for selection in environment_selections {
        selection.cwd = worktree_path.clone();
    }
}

fn append_worktree_isolation_notice(
    message: &str,
    worktree_path: &AbsolutePathBuf,
    parent_cwd: &AbsolutePathBuf,
) -> String {
    format!(
        "{message}\n\n---\nYou are running in an isolated git worktree at {} (a separate working copy of the repo). Changes you make here do NOT affect the main working directory ({}). Work normally; the worktree will be removed automatically if unchanged, or preserved for review if changed.",
        worktree_path.display(),
        parent_cwd.display()
    )
}

impl CoreToolRuntime for Handler {
    fn matches_kind(&self, payload: &ToolPayload) -> bool {
        matches!(payload, ToolPayload::Function { .. })
    }
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnAgentArgs {
    message: String,
    task_name: String,
    agent_type: Option<String>,
    model: Option<String>,
    reasoning_effort: Option<ReasoningEffort>,
    service_tier: Option<String>,
    isolation: Option<String>,
    fork_turns: Option<String>,
    fork_context: Option<bool>,
    #[serde(
        default,
        alias = "output_schema",
        alias = "outputSchema",
        alias = "json_schema",
        alias = "jsonSchema"
    )]
    schema: Option<JsonValue>,
}

impl SpawnAgentArgs {
    fn fork_mode(&self) -> Result<Option<SpawnAgentForkMode>, FunctionCallError> {
        if self.fork_context.is_some() {
            return Err(FunctionCallError::RespondToModel(
                "fork_context is not supported in MultiAgentV2; use fork_turns instead".to_string(),
            ));
        }

        let fork_turns = self
            .fork_turns
            .as_deref()
            .map(str::trim)
            .filter(|fork_turns| !fork_turns.is_empty())
            .unwrap_or("all");

        if fork_turns.eq_ignore_ascii_case("none") {
            return Ok(None);
        }
        if fork_turns.eq_ignore_ascii_case("all") {
            return Ok(Some(SpawnAgentForkMode::FullHistory));
        }

        let last_n_turns = fork_turns.parse::<usize>().map_err(|_| {
            FunctionCallError::RespondToModel(
                "fork_turns must be `none`, `all`, or a positive integer string".to_string(),
            )
        })?;
        if last_n_turns == 0 {
            return Err(FunctionCallError::RespondToModel(
                "fork_turns must be `none`, `all`, or a positive integer string".to_string(),
            ));
        }

        Ok(Some(SpawnAgentForkMode::LastNTurns(last_n_turns)))
    }
}

#[derive(Debug, Serialize)]
#[serde(untagged)]
pub(crate) enum SpawnAgentResult {
    WithNickname {
        task_name: String,
        nickname: Option<String>,
        #[serde(skip_serializing_if = "is_false")]
        workflow_live_transcript: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
    },
    HiddenMetadata {
        task_name: String,
        #[serde(skip_serializing_if = "is_false")]
        workflow_live_transcript: bool,
        #[serde(skip_serializing_if = "Option::is_none")]
        tool_use_id: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        worktree_path: Option<String>,
    },
}

fn is_false(value: &bool) -> bool {
    !*value
}

impl ToolOutput for SpawnAgentResult {
    fn log_preview(&self) -> String {
        tool_output_json_text(self, "spawn_agent")
    }

    fn success_for_logging(&self) -> bool {
        true
    }

    fn to_response_item(&self, call_id: &str, payload: &ToolPayload) -> ResponseInputItem {
        tool_output_response_item(call_id, payload, self, Some(true), "spawn_agent")
    }

    fn code_mode_result(&self, _payload: &ToolPayload) -> JsonValue {
        tool_output_code_mode_result(self, "spawn_agent")
    }
}
