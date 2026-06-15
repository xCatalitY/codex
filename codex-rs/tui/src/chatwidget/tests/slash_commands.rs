use super::*;
use crate::bottom_pane::slash_commands::ServiceTierCommand;
use pretty_assertions::assert_eq;
use serial_test::serial;

fn force_pet_image_support(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Supported(
        crate::pets::ImageProtocol::Kitty,
    ));
}

fn force_tmux_pet_image_unsupported(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Unsupported(
        crate::pets::PetImageUnsupportedReason::Tmux,
    ));
}

fn force_terminal_pet_image_unsupported(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Unsupported(
        crate::pets::PetImageUnsupportedReason::Terminal,
    ));
}

fn force_old_iterm2_pet_image_unsupported(chat: &mut ChatWidget) {
    chat.set_pet_image_support_for_tests(crate::pets::PetImageSupport::Unsupported(
        crate::pets::PetImageUnsupportedReason::Iterm2TooOld,
    ));
}

fn fast_tier_command() -> ServiceTierCommand {
    ServiceTierCommand {
        id: ServiceTier::Fast.request_value().to_string(),
        name: "fast".to_string(),
        description: "Fastest inference with increased plan usage".to_string(),
    }
}

fn complete_turn_with_message(chat: &mut ChatWidget, turn_id: &str, message: Option<&str>) {
    if let Some(message) = message {
        complete_assistant_message(
            chat,
            &format!("{turn_id}-message"),
            message,
            Some(MessagePhase::FinalAnswer),
        );
    }
    handle_turn_completed(chat, turn_id, /*duration_ms*/ None);
}

fn submit_composer_text(chat: &mut ChatWidget, text: &str) {
    chat.bottom_pane
        .set_composer_text(text.to_string(), Vec::new(), Vec::new());
    submit_current_composer(chat);
}

fn submit_current_composer(chat: &mut ChatWidget) {
    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
}

fn queue_composer_text_with_tab(chat: &mut ChatWidget, text: &str) {
    chat.bottom_pane
        .set_composer_text(text.to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Tab, KeyModifiers::NONE));
}

fn recall_latest_after_clearing(chat: &mut ChatWidget) -> String {
    chat.bottom_pane
        .set_composer_text(String::new(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    chat.bottom_pane.composer_text()
}

fn next_add_to_history_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<AppEvent>) -> String {
    loop {
        match rx.try_recv() {
            Ok(AppEvent::AppendMessageHistoryEntry { text, .. }) => return text,
            Ok(_) => continue,
            Err(TryRecvError::Empty) => {
                panic!("expected AppendMessageHistoryEntry event but queue was empty")
            }
            Err(TryRecvError::Disconnected) => {
                panic!("expected AppendMessageHistoryEntry event but channel closed")
            }
        }
    }
}

#[tokio::test]
async fn service_tier_commands_lowercase_catalog_names() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    let mut preset = get_available_model(&chat, "gpt-5.4");
    let expected_description = preset
        .service_tiers
        .iter()
        .find(|tier| tier.id == ServiceTier::Fast.request_value())
        .expect("fast tier")
        .description
        .clone();
    preset
        .service_tiers
        .iter_mut()
        .find(|tier| tier.id == ServiceTier::Fast.request_value())
        .expect("fast tier")
        .name = "Fast".to_string();
    chat.model_catalog = std::sync::Arc::new(ModelCatalog::new(vec![preset]));

    assert_eq!(
        chat.current_model_service_tier_commands(),
        vec![ServiceTierCommand {
            id: ServiceTier::Fast.request_value().to_string(),
            name: "fast".to_string(),
            description: expected_description,
        }]
    );
}

#[tokio::test]
async fn slash_compact_eagerly_queues_follow_up_before_turn_start() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Compact);

    assert!(chat.bottom_pane.is_task_running());
    match rx.try_recv() {
        Ok(AppEvent::CodexOp(Op::Compact)) => {}
        other => panic!("expected compact op to be submitted, got {other:?}"),
    }

    chat.bottom_pane.set_composer_text(
        "queued before compact turn start".to_string(),
        Vec::new(),
        Vec::new(),
    );
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert!(chat.input_queue.pending_steers.is_empty());
    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue.queued_user_messages.front().unwrap().text,
        "queued before compact turn start"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn queued_slash_compact_dispatches_after_active_turn() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/compact");

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue
            .queued_user_messages
            .front()
            .unwrap()
            .action,
        QueuedInputAction::ParseSlash
    );
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::CodexOp(Op::Compact))),
        "expected queued /compact to submit compact op; events: {events:?}"
    );
}

#[tokio::test]
async fn queued_slash_review_with_args_dispatches_after_active_turn() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/review check regressions");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    match op_rx.try_recv() {
        Ok(Op::Review { target }) => assert_eq!(
            target,
            ReviewTarget::Custom {
                instructions: "check regressions".to_string(),
            }
        ),
        other => panic!("expected queued /review to submit review op, got {other:?}"),
    }
}

#[tokio::test]
async fn queued_slash_review_with_args_restores_for_edit() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/review check regressions");
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::ALT));

    assert_eq!(
        chat.bottom_pane.composer_text(),
        "/review check regressions"
    );
}

#[tokio::test]
async fn queued_bang_shell_dispatches_after_active_turn() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "!echo hi");

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_eq!(
        chat.input_queue
            .queued_user_messages
            .front()
            .unwrap()
            .action,
        QueuedInputAction::RunShell
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    match op_rx.try_recv() {
        Ok(Op::RunUserShellCommand { command }) => assert_eq!(command, "echo hi"),
        other => panic!("expected queued shell command op, got {other:?}"),
    }
    assert_eq!(next_add_to_history_event(&mut rx), "!echo hi");
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_empty_bang_shell_reports_help_when_dequeued_and_drains_next_input() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "!");
    queue_composer_text_with_tab(&mut chat, "hello after help");

    assert!(drain_insert_history(&mut rx).is_empty());

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(USER_SHELL_COMMAND_HELP_TITLE),
        "expected delayed shell help, got {rendered:?}"
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after help".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after empty shell command, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_bang_shell_waits_for_user_shell_completion_before_next_input() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "!echo hi");
    queue_composer_text_with_tab(&mut chat, "hello after shell");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    match op_rx.try_recv() {
        Ok(Op::RunUserShellCommand { command }) => assert_eq!(command, "echo hi"),
        other => panic!("expected queued shell command op, got {other:?}"),
    }
    assert_eq!(next_add_to_history_event(&mut rx), "!echo hi");
    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);

    let begin = begin_exec_with_source(
        &mut chat,
        "user-shell-echo",
        "echo hi",
        ExecCommandSource::UserShell,
    );
    end_exec(&mut chat, begin, "hi\n", "", /*exit_code*/ 0);

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after shell".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after shell completion, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

async fn assert_cancelled_queued_menu_drains_next_input(command: &str, expected_popup_text: &str) {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, command);
    queue_composer_text_with_tab(&mut chat, "hello after menu");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains(expected_popup_text),
        "expected {command} menu to open; popup:\n{popup}"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after menu".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after cancelling {command}, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_slash_menu_cancel_drains_next_input() {
    assert_cancelled_queued_menu_drains_next_input("/model", "Select Model").await;
    assert_cancelled_queued_menu_drains_next_input("/permissions", "Update Model Permissions")
        .await;
}

#[tokio::test]
async fn queued_slash_menu_selection_drains_next_input() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/permissions");
    queue_composer_text_with_tab(&mut chat, "hello after selection");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(
        popup.contains("Update Model Permissions"),
        "expected permissions menu to open; popup:\n{popup}"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after selection".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after permissions selection, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_bare_rename_drains_next_input_after_name_update() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/rename");
    queue_composer_text_with_tab(&mut chat, "hello after rename");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Name thread"));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    chat.handle_paste("Queued rename".to_string());
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::SetThreadName { name }) if name == "Queued rename"
        )),
        "expected rename prompt to submit thread name; events: {events:?}"
    );

    chat.handle_server_notification(
        ServerNotification::ThreadNameUpdated(
            codex_app_server_protocol::ThreadNameUpdatedNotification {
                thread_id: thread_id.to_string(),
                thread_name: Some("Queued rename".to_string()),
            },
        ),
        /*replay_kind*/ None,
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after rename".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message after /rename, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_inline_rename_does_not_drain_again_before_turn_started() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/rename Queued rename");
    queue_composer_text_with_tab(&mut chat, "first after rename");
    queue_composer_text_with_tab(&mut chat, "second after rename");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::SetThreadName { name }) if name == "Queued rename"
        )),
        "expected queued /rename to submit thread name; events: {events:?}"
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "first after rename".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected first queued message after /rename, got {other:?}"),
    }
    assert!(events.iter().any(|event| matches!(
        event,
        AppEvent::AppendMessageHistoryEntry { text, .. } if text == "first after rename"
    )));
    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["second after rename"]
    );
    let input_state = chat.capture_thread_input_state().unwrap();
    assert!(input_state.user_turn_pending_start);
    chat.restore_thread_input_state(/*input_state*/ None);
    assert!(!chat.input_queue.user_turn_pending_start);
    chat.restore_thread_input_state(Some(input_state));
    assert!(chat.input_queue.user_turn_pending_start);
    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["second after rename"]
    );

    chat.handle_server_notification(
        ServerNotification::ThreadNameUpdated(
            codex_app_server_protocol::ThreadNameUpdatedNotification {
                thread_id: thread_id.to_string(),
                thread_name: Some("Queued rename".to_string()),
            },
        ),
        /*replay_kind*/ None,
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
    assert_eq!(
        chat.queued_user_message_texts(),
        vec!["second after rename"]
    );

    handle_turn_started(&mut chat, "turn-2");
    complete_turn_with_message(&mut chat, "turn-2", Some("done"));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "second after rename".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected second queued message after turn complete, got {other:?}"),
    }
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn queued_unknown_slash_reports_error_when_dequeued() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/does-not-exist");

    assert!(drain_insert_history(&mut rx).is_empty());

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Unrecognized command '/does-not-exist'"),
        "expected delayed slash error, got {rendered:?}"
    );
    assert!(chat.input_queue.queued_user_messages.is_empty());
}

#[tokio::test]
async fn ctrl_d_quits_without_prompt() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));
    assert_matches!(rx.try_recv(), Ok(AppEvent::Exit(ExitMode::ShutdownFirst)));
}

#[tokio::test]
async fn ctrl_d_with_modal_open_does_not_quit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.open_approvals_popup();
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL));

    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn slash_init_does_not_depend_on_loaded_instruction_sources() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.instruction_source_paths = vec![chat.config.cwd.join("project-instructions.md")];

    submit_composer_text(&mut chat, "/init");

    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert!(drain_insert_history(&mut rx).is_empty());
    assert_eq!(recall_latest_after_clearing(&mut chat), "/init");
}

#[tokio::test]
async fn bare_slash_command_is_available_from_local_recall_after_dispatch() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/diff");

    let _ = drain_insert_history(&mut rx);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "/diff");
}

#[tokio::test]
async fn inline_slash_command_is_available_from_local_recall_after_dispatch() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/rename Better title");

    let _ = drain_insert_history(&mut rx);
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "/rename Better title");
}

#[tokio::test]
async fn goal_slash_command_emits_set_goal_event() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let command = "/goal --tokens 98.5K improve benchmark coverage";

    submit_composer_text(&mut chat, command);

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        mode,
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "--tokens 98.5K improve benchmark coverage");
    assert_eq!(mode, crate::app_event::ThreadGoalSetMode::ConfirmIfExists);
    assert_no_submit_op(&mut op_rx);
    assert_eq!(recall_latest_after_clearing(&mut chat), command);
}

#[tokio::test]
async fn goal_slash_command_uses_plain_text_for_mentions() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        "/goal use $figma for the mockup".to_string(),
        Vec::new(),
        Vec::new(),
        vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::End, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        ..
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "use $figma for the mockup");
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn goal_slash_command_drops_attached_images() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/goal.png".to_string();
    let local_image = PathBuf::from("/tmp/goal-local.png");
    let placeholder = "[Image #2]";
    let command = format!("/goal describe {placeholder}");
    let placeholder_start = command.find(placeholder).expect("placeholder in command");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane.set_composer_text(
        command,
        vec![TextElement::new(
            (placeholder_start..placeholder_start + placeholder.len()).into(),
            Some(placeholder.to_string()),
        )],
        vec![local_image],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        ..
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "describe [Image #2]");
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn bare_goal_slash_command_drains_pending_submission_state() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    let remote_url = "https://example.com/goal-menu.png".to_string();
    let local_image = PathBuf::from("/tmp/goal-menu-local.png");
    chat.set_remote_image_urls(vec![remote_url]);
    chat.bottom_pane
        .set_composer_text("/goal".to_string(), Vec::new(), vec![local_image]);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenThreadGoalMenu { thread_id: opened }) if opened == thread_id
    );
    assert!(chat.remote_image_urls().is_empty());
    assert!(chat.bottom_pane.composer_local_image_paths().is_empty());
}

#[tokio::test]
async fn goal_control_slash_commands_emit_goal_events() {
    let cases = [
        ("/goal clear", None),
        ("/goal pause", Some(AppThreadGoalStatus::Paused)),
        ("/goal resume", Some(AppThreadGoalStatus::Active)),
    ];

    for (command, status) in cases {
        let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
        let thread_id = ThreadId::new();
        chat.thread_id = Some(thread_id);

        submit_composer_text(&mut chat, command);

        match status {
            Some(status) => {
                let event = rx.try_recv().expect("expected goal status event");
                let AppEvent::SetThreadGoalStatus {
                    thread_id: actual_thread_id,
                    status: actual_status,
                } = event
                else {
                    panic!("expected SetThreadGoalStatus, got {event:?}");
                };
                assert_eq!(actual_thread_id, thread_id);
                assert_eq!(actual_status, status);
            }
            None => {
                let event = rx.try_recv().expect("expected clear goal event");
                let AppEvent::ClearThreadGoal {
                    thread_id: actual_thread_id,
                } = event
                else {
                    panic!("expected ClearThreadGoal, got {event:?}");
                };
                assert_eq!(actual_thread_id, thread_id);
            }
        }
    }
}

#[tokio::test]
async fn goal_control_slash_command_without_thread_shows_full_usage() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);

    submit_composer_text(&mut chat, "/goal pause");

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected goal usage message");
    insta::assert_snapshot!(
        lines_to_single_string(&cells[0]),
        @"• Usage: /goal [<objective>|clear|edit|pause|resume] The session must start before you can change a goal."
    );
}

#[tokio::test]
async fn goal_edit_slash_command_opens_goal_editor() {
    for thread_id in [Some(ThreadId::new()), None] {
        let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
        chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
        chat.thread_id = thread_id;

        submit_composer_text(&mut chat, "/goal edit");

        let event = rx.try_recv().expect("expected goal editor event");
        let AppEvent::OpenThreadGoalEditor {
            thread_id: actual_thread_id,
        } = event
        else {
            panic!("expected OpenThreadGoalEditor, got {event:?}");
        };
        assert_eq!(actual_thread_id, thread_id);
        assert_no_submit_op(&mut op_rx);
    }
}

#[tokio::test]
async fn queued_goal_slash_command_emits_set_goal_event_after_thread_starts() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let command = "/goal improve benchmark coverage";

    submit_composer_text(&mut chat, command);
    assert_eq!(chat.input_queue.queued_user_messages.len(), 1);
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.maybe_send_next_queued_input();

    let event = rx.try_recv().expect("expected goal objective event");
    let AppEvent::SetThreadGoalObjective {
        thread_id: actual_thread_id,
        objective,
        ..
    } = event
    else {
        panic!("expected SetThreadGoalObjective, got {event:?}");
    };
    assert_eq!(actual_thread_id, thread_id);
    assert_eq!(objective, "improve benchmark coverage");
    assert_no_submit_op(&mut op_rx);
}

#[tokio::test]
async fn queued_goal_slash_command_preserves_current_draft_metadata() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let command = "/goal improve benchmark coverage";

    submit_composer_text(&mut chat, command);
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));

    let remote_url = "https://example.com/current-draft.png".to_string();
    let local_image = PathBuf::from("/tmp/current-draft-local.png");
    let placeholder = "[Image #3]";
    let draft = format!("draft with {placeholder}");
    let placeholder_start = draft.find(placeholder).expect("placeholder in draft");
    chat.set_remote_image_urls(vec![remote_url.clone()]);
    chat.bottom_pane.set_composer_text(
        draft.clone(),
        vec![TextElement::new(
            (placeholder_start..placeholder_start + placeholder.len()).into(),
            Some(placeholder.to_string()),
        )],
        vec![local_image.clone()],
    );

    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.maybe_send_next_queued_input();

    let event = rx.try_recv().expect("expected goal objective event");
    assert_matches!(
        event,
        AppEvent::SetThreadGoalObjective {
            thread_id: actual_thread_id,
            ..
        } if actual_thread_id == thread_id
    );
    assert_no_submit_op(&mut op_rx);
    assert_eq!(chat.bottom_pane.composer_text(), draft);
    assert_eq!(chat.remote_image_urls(), vec![remote_url]);
    assert_eq!(
        chat.bottom_pane.composer_local_image_paths(),
        vec![local_image]
    );
}

#[tokio::test]
async fn restored_queued_goal_slash_command_emits_set_goal_event() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    let command = "/goal improve benchmark coverage";

    submit_composer_text(&mut chat, command);
    let input_state = chat
        .capture_thread_input_state()
        .expect("expected queued input state");

    let (mut restored_chat, mut restored_rx, mut restored_op_rx) =
        make_chatwidget_manual(/*model_override*/ None).await;
    restored_chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    restored_chat.restore_thread_input_state(Some(input_state));
    let thread_id = ThreadId::new();
    restored_chat.thread_id = Some(thread_id);
    restored_chat.maybe_send_next_queued_input();

    let event = restored_rx
        .try_recv()
        .expect("expected goal objective event");
    assert_matches!(
        event,
        AppEvent::SetThreadGoalObjective {
            thread_id: actual_thread_id,
            ..
        } if actual_thread_id == thread_id
    );
    assert_no_submit_op(&mut restored_op_rx);
}

#[test]
fn merged_history_record_preserves_raw_text_and_rebased_elements() {
    let first = UserMessage {
        text: "Ask $figma".to_string(),
        local_images: Vec::new(),
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new((4..10).into(), Some("$figma".to_string()))],
        mention_bindings: vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }],
    };
    let second = UserMessage::from("internal prompt");

    let (_message, history_record) = merge_user_messages_with_history_record(vec![
        (first, UserMessageHistoryRecord::UserMessageText),
        (
            second,
            UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
                text: "/goal inspect [Image #1]".to_string(),
                text_elements: vec![TextElement::new(
                    (14..24).into(),
                    Some("[Image #1]".to_string()),
                )],
            }),
        ),
    ]);

    assert_eq!(
        history_record,
        UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
            text: "Ask $figma\n/goal inspect [Image #1]".to_string(),
            text_elements: vec![
                TextElement::new((4..10).into(), Some("$figma".to_string())),
                TextElement::new((25..35).into(), Some("[Image #1]".to_string())),
            ],
        })
    );
}

#[test]
fn merged_history_record_remaps_override_image_placeholders() {
    let first_placeholder = "[Image #1]";
    let second_placeholder = "[Image #1]";
    let first = UserMessage {
        text: format!("first {first_placeholder}"),
        local_images: vec![LocalImageAttachment {
            placeholder: first_placeholder.to_string(),
            path: PathBuf::from("/tmp/first.png"),
        }],
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new(
            (6..16).into(),
            Some(first_placeholder.to_string()),
        )],
        mention_bindings: Vec::new(),
    };
    let second = UserMessage {
        text: format!("internal {second_placeholder}"),
        local_images: vec![LocalImageAttachment {
            placeholder: second_placeholder.to_string(),
            path: PathBuf::from("/tmp/second.png"),
        }],
        remote_image_urls: Vec::new(),
        text_elements: vec![TextElement::new(
            (9..19).into(),
            Some(second_placeholder.to_string()),
        )],
        mention_bindings: Vec::new(),
    };

    let (message, history_record) = merge_user_messages_with_history_record(vec![
        (first, UserMessageHistoryRecord::UserMessageText),
        (
            second,
            UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
                text: format!("goal {second_placeholder}"),
                text_elements: vec![TextElement::new(
                    (5..15).into(),
                    Some(second_placeholder.to_string()),
                )],
            }),
        ),
    ]);

    assert_eq!(message.text, "first [Image #1]\ninternal [Image #2]");
    assert_eq!(
        message.text_elements,
        vec![
            TextElement::new((6..16).into(), Some("[Image #1]".to_string())),
            TextElement::new((26..36).into(), Some("[Image #2]".to_string())),
        ]
    );
    assert_eq!(
        message
            .local_images
            .iter()
            .map(|image| image.placeholder.as_str())
            .collect::<Vec<_>>(),
        vec!["[Image #1]", "[Image #2]"]
    );
    assert_eq!(
        history_record,
        UserMessageHistoryRecord::Override(UserMessageHistoryOverride {
            text: "first [Image #1]\ngoal [Image #2]".to_string(),
            text_elements: vec![
                TextElement::new((6..16).into(), Some("[Image #1]".to_string())),
                TextElement::new((22..32).into(), Some("[Image #2]".to_string())),
            ],
        })
    );
}

#[tokio::test]
async fn interrupted_merged_message_history_encodes_mentions_once() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.on_task_started();
    chat.on_agent_message_delta("Final answer line\n".to_string());
    let text = "use $figma now";
    chat.bottom_pane.set_composer_text_with_mention_bindings(
        text.to_string(),
        Vec::new(),
        Vec::new(),
        vec![MentionBinding {
            sigil: '$',
            mention: "figma".to_string(),
            path: "app://figma".to_string(),
        }],
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            let [
                UserInput::Text {
                    text: submitted, ..
                },
            ] = items.as_slice()
            else {
                panic!("expected text item, got {items:?}");
            };
            assert_eq!(submitted, text);
        }
        other => panic!("expected user turn, got {other:?}"),
    }
    let encoded = "use [$figma](app://figma) now";
    assert_eq!(next_add_to_history_event(&mut rx), encoded);

    chat.handle_key_event(KeyEvent::new(KeyCode::Esc, KeyModifiers::NONE));
    next_interrupt_op(&mut op_rx);
    chat.on_interrupted_turn(TurnAbortReason::Interrupted);

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            let [
                UserInput::Text {
                    text: submitted, ..
                },
            ] = items.as_slice()
            else {
                panic!("expected resubmitted text item, got {items:?}");
            };
            assert_eq!(submitted, text);
        }
        other => panic!("expected resubmitted user turn, got {other:?}"),
    }
    assert_eq!(next_add_to_history_event(&mut rx), encoded);
}

#[tokio::test]
async fn slash_rename_prefills_existing_thread_name() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_name = Some("Current project title".to_string());

    chat.dispatch_command(SlashCommand::Rename);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_rename_prefilled_prompt", popup);

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::CodexOp(Op::SetThreadName { name })) if name == "Current project title"
    );
}

#[tokio::test]
async fn slash_rename_without_existing_thread_name_starts_empty() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Rename);

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("Name thread"));
    assert!(popup.contains("Type a name and press Enter"));

    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn usage_error_slash_command_is_available_from_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;

    submit_composer_text(&mut chat, "/raw maybe");

    assert_eq!(chat.bottom_pane.composer_text(), "");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /raw [on|off]"),
        "expected usage message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/raw maybe");
}

#[tokio::test]
async fn unrecognized_slash_command_is_not_added_to_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/does-not-exist");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Unrecognized command '/does-not-exist'"),
        "expected unrecognized-command message, got: {rendered:?}"
    );
    assert_eq!(chat.bottom_pane.composer_text(), "/does-not-exist");
    assert_eq!(recall_latest_after_clearing(&mut chat), "");
}

#[tokio::test]
async fn unavailable_slash_command_is_available_from_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    submit_composer_text(&mut chat, "/model");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("'/model' is disabled while a task is in progress."),
        "expected disabled-command message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/model");
}

#[tokio::test]
async fn no_op_stub_slash_command_is_available_from_local_recall() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/debug-m-drop");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Memory maintenance"),
        "expected stub message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/debug-m-drop");
}

fn write_workflow_run_snapshot(chat: &ChatWidget, file_name: &str, value: serde_json::Value) {
    let runs_dir = chat.config.codex_home.join("workflow-runs").to_path_buf();
    std::fs::create_dir_all(&runs_dir).expect("create workflow runs dir");
    std::fs::write(runs_dir.join(file_name), value.to_string()).expect("write workflow snapshot");
}

fn write_workflow_transcript_snapshot(chat: &ChatWidget, run_id: &str, value: serde_json::Value) {
    let transcript_dir = chat
        .config
        .codex_home
        .join("workflow-runs")
        .join(run_id)
        .join("transcripts");
    std::fs::create_dir_all(&transcript_dir).expect("create workflow transcript dir");
    std::fs::write(transcript_dir.join("run.json"), value.to_string())
        .expect("write workflow transcript snapshot");
}

fn write_active_workflow_run_marker(chat: &ChatWidget, file_name: &str, value: serde_json::Value) {
    let active_dir = chat.config.codex_home.join("workflow-runs").join("active");
    std::fs::create_dir_all(&active_dir).expect("create active workflow runs dir");
    std::fs::write(active_dir.join(file_name), value.to_string())
        .expect("write active workflow marker");
}

fn write_workflow_definition(path: &std::path::Path, metadata_name: &str, description: &str) {
    let parent = path.parent().expect("workflow file parent");
    std::fs::create_dir_all(parent).expect("create workflow definition dir");
    std::fs::write(
        path,
        format!(
            "export const meta = {{ name: '{metadata_name}', description: '{description}' }};\nphase('test');\n"
        ),
    )
    .expect("write workflow definition");
}

fn write_rich_workflow_definition(path: &std::path::Path) {
    let parent = path.parent().expect("workflow file parent");
    std::fs::create_dir_all(parent).expect("create workflow definition dir");
    std::fs::write(
        path,
        "export const meta = {
  name: 'release',
  description: 'Project release',
  whenToUse: 'Use when publishing a release channel',
  inputSchema: { type: 'object', properties: { channel: { type: 'string' } } },
  phases: [{ title: 'Build' }, { title: 'Publish', model: 'xhigh' }],
};
phase('test');
",
    )
    .expect("write workflow definition");
}

#[tokio::test]
async fn slash_config_opens_workflow_settings_view() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Config);

    assert_eq!(chat.bottom_pane.active_view_id(), Some("workflow-settings"));
}

#[tokio::test]
async fn workflow_settings_policy_items_include_discovered_and_configured_names() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let project_dir = chat.config.codex_home.join("project-workflows");
    let plugin_dir = chat.config.codex_home.join("plugin-workflows");
    chat.config.workflows.workflow_dirs = vec![project_dir.clone()];
    chat.config.workflows.plugin_workflow_dirs =
        vec![crate::legacy_core::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: plugin_dir.clone(),
        }];
    chat.config.workflows.named.insert(
        "release".to_string(),
        codex_config::types::WorkflowDefinitionConfig {
            enabled: Some(false),
            approval: Some(codex_config::types::WorkflowApproval::Ask),
        },
    );
    chat.config.workflows.named.insert(
        "manual-only".to_string(),
        codex_config::types::WorkflowDefinitionConfig {
            enabled: Some(true),
            approval: Some(codex_config::types::WorkflowApproval::Deny),
        },
    );
    write_workflow_definition(
        project_dir.join("release.js").as_path(),
        "release",
        "Project release",
    );
    write_workflow_definition(
        project_dir.join("docs").join("workflow.js").as_path(),
        "docs",
        "Docs workflow",
    );
    write_workflow_definition(
        plugin_dir.join("release.js").as_path(),
        "release",
        "Plugin release",
    );

    let items = chat.workflow_policy_items_for_settings();
    let summary = items
        .into_iter()
        .map(|item| (item.name, item.enabled, item.approval))
        .collect::<Vec<_>>();

    assert_eq!(
        summary,
        vec![
            ("docs".to_string(), None, None),
            (
                "manual-only".to_string(),
                Some(true),
                Some(codex_config::types::WorkflowApproval::Deny)
            ),
            (
                "release".to_string(),
                Some(false),
                Some(codex_config::types::WorkflowApproval::Ask)
            ),
            ("sample:release".to_string(), None, None),
        ]
    );
}

#[tokio::test]
async fn slash_workflows_lists_available_definitions_and_shadowing() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let project_dir = chat.config.codex_home.join("project-workflows");
    let global_dir = chat.config.codex_home.join("global-workflows");
    let system_dir = chat.config.codex_home.join("workflows/.system");
    let plugin_dir = chat.config.codex_home.join("plugin-workflows");
    chat.config.workflows.workflow_dirs =
        vec![project_dir.clone(), global_dir.clone(), system_dir.clone()];
    chat.config.workflows.plugin_workflow_dirs =
        vec![crate::legacy_core::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: plugin_dir.clone(),
        }];
    chat.config.workflows.named.insert(
        "release".to_string(),
        codex_config::types::WorkflowDefinitionConfig {
            enabled: Some(false),
            approval: Some(codex_config::types::WorkflowApproval::Ask),
        },
    );
    chat.config.workflows.named.insert(
        "sample:release".to_string(),
        codex_config::types::WorkflowDefinitionConfig {
            enabled: Some(true),
            approval: Some(codex_config::types::WorkflowApproval::Allow),
        },
    );

    write_rich_workflow_definition(project_dir.join("release.js").as_path());
    write_workflow_definition(
        global_dir.join("release.js").as_path(),
        "release",
        "Global release",
    );
    write_workflow_definition(
        global_dir.join("docs").join("workflow.js").as_path(),
        "docs-meta",
        "Docs workflow",
    );
    write_workflow_definition(
        system_dir.join("builtin.js").as_path(),
        "builtin",
        "Builtin workflow",
    );
    write_workflow_definition(
        plugin_dir.join("release.js").as_path(),
        "release",
        "Plugin release",
    );
    std::fs::write(project_dir.join("broken.js"), "phase('missing meta');")
        .expect("write invalid workflow definition");
    std::fs::write(
        project_dir.join("bad-schema.js"),
        "export const meta = { name: 'bad-schema', description: 'Bad schema', inputSchema: makeSchema() };\nphase('test');\n",
    )
    .expect("write invalid workflow schema definition");

    chat.dispatch_command(SlashCommand::Workflows);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Available workflows:"), "{rendered}");
    assert!(
        rendered.contains("- `release` - Project release"),
        "{rendered}"
    );
    assert!(rendered.contains("source: [Workflow]"), "{rendered}");
    assert!(
        rendered.contains("when: Use when publishing a release channel"),
        "{rendered}"
    );
    assert!(
        rendered.contains("input: { type: 'object', properties: { channel: { type: 'string' } } }"),
        "{rendered}"
    );
    assert!(
        rendered.contains("phases: Build, Publish [xhigh]"),
        "{rendered}"
    );
    assert!(
        rendered.contains("policy: disabled; approval: ask"),
        "{rendered}"
    );
    assert!(
        !rendered.contains("- `release` - Global release"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- `docs` (meta `docs-meta`) - Docs workflow"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- `builtin` - Builtin workflow"),
        "{rendered}"
    );
    assert!(rendered.contains("source: [System Workflow]"), "{rendered}");
    assert!(
        rendered.contains("- `sample:release` (meta `release`) - Plugin release"),
        "{rendered}"
    );
    assert!(rendered.contains("source: [Plugin Workflow]"), "{rendered}");
    assert!(
        rendered.contains("policy: enabled; approval: allow"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Shadowed workflow definitions: 1"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Skipped invalid workflow definitions: 2"),
        "{rendered}"
    );
}

#[tokio::test]
async fn saved_workflow_slash_command_submits_invocation_prompt() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let workflow_dir = chat.config.codex_home.join("workflows");
    chat.config.workflows.workflow_dirs = vec![workflow_dir.clone()];
    write_rich_workflow_definition(workflow_dir.join("release.js").as_path());
    chat.sync_workflow_slash_commands();

    submit_composer_text(&mut chat, "/release ship alpha");

    let rendered_history = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered_history.contains("Unrecognized command"),
        "{rendered_history}"
    );
    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text {
                text,
                text_elements,
            } = &items[0]
            else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(text.contains("Run the saved workflow `release`"), "{text}");
            assert!(text.contains("workflow tool with name `release`"), "{text}");
            assert!(text.contains("ship alpha"), "{text}");
            assert!(
                text.contains(
                    "Interpret these user arguments according to the workflow input schema below"
                ),
                "{text}"
            );
            assert!(
                text.contains("set `args` to the resulting JSON value"),
                "{text}"
            );
            assert!(!text.contains("pass these user arguments"), "{text}");
            assert!(text.contains("Workflow input schema:"), "{text}");
            assert!(
                text.contains("{ type: 'object', properties: { channel: { type: 'string' } } }"),
                "{text}"
            );
            assert!(text_elements.is_empty(), "{text_elements:?}");
        }
        other => panic!("expected workflow slash command to submit user turn, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_workflows_run_submits_named_workflow_invocation_prompt() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let workflow_dir = chat.config.codex_home.join("workflows");
    chat.config.workflows.workflow_dirs = vec![workflow_dir.clone()];
    write_rich_workflow_definition(workflow_dir.join("release.js").as_path());

    submit_composer_text(&mut chat, r#"/workflows run release {"channel":"alpha"}"#);

    let rendered_history = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered_history.contains("Unrecognized command"),
        "{rendered_history}"
    );
    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text {
                text,
                text_elements,
            } = &items[0]
            else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(text.contains("Run the saved workflow `release`"), "{text}");
            assert!(text.contains("workflow tool with name `release`"), "{text}");
            assert!(
                text.contains("set `args` to this exact JSON value"),
                "{text}"
            );
            assert!(text.contains(r#""channel": "alpha""#), "{text}");
            assert!(!text.contains("pass these user arguments"), "{text}");
            assert!(text.contains("Workflow input schema:"), "{text}");
            assert!(
                text.contains("{ type: 'object', properties: { channel: { type: 'string' } } }"),
                "{text}"
            );
            assert!(text_elements.is_empty(), "{text_elements:?}");
        }
        other => panic!("expected /workflows run to submit user turn, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_workflows_run_unwraps_shell_quoted_json_args() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let workflow_dir = chat.config.codex_home.join("workflows");
    chat.config.workflows.workflow_dirs = vec![workflow_dir.clone()];
    write_rich_workflow_definition(workflow_dir.join("release.js").as_path());

    submit_composer_text(&mut chat, r#"/workflows run release '{"channel":"alpha"}'"#);

    let rendered_history = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered_history.contains("Unrecognized command"),
        "{rendered_history}"
    );
    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text {
                text,
                text_elements,
            } = &items[0]
            else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(
                text.contains("set `args` to this exact JSON value"),
                "{text}"
            );
            assert!(text.contains(r#""channel": "alpha""#), "{text}");
            assert!(!text.contains("Interpret these user arguments"), "{text}");
            assert!(text_elements.is_empty(), "{text_elements:?}");
        }
        other => panic!("expected /workflows run to submit user turn, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_workflows_run_submits_plugin_workflow_invocation_prompt() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let plugin_dir = chat.config.codex_home.join("plugin-workflows");
    write_rich_workflow_definition(plugin_dir.join("release.js").as_path());
    chat.on_plugin_mentions_loaded(
        None,
        vec![crate::legacy_core::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: plugin_dir,
        }],
    );

    submit_composer_text(
        &mut chat,
        r#"/workflows run sample:release {"channel":"alpha"}"#,
    );

    let rendered_history = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered_history.contains("Unrecognized command"),
        "{rendered_history}"
    );
    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text {
                text,
                text_elements,
            } = &items[0]
            else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(
                text.contains("Run the saved workflow `sample:release`"),
                "{text}"
            );
            assert!(
                text.contains("workflow tool with name `sample:release`"),
                "{text}"
            );
            assert!(
                text.contains("set `args` to this exact JSON value"),
                "{text}"
            );
            assert!(text.contains(r#""channel": "alpha""#), "{text}");
            assert!(text.contains("Workflow input schema:"), "{text}");
            assert!(
                text.contains("{ type: 'object', properties: { channel: { type: 'string' } } }"),
                "{text}"
            );
            assert!(text_elements.is_empty(), "{text_elements:?}");
        }
        other => panic!("expected /workflows run to submit plugin workflow turn, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_workflows_run_rejects_disabled_named_workflow() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let workflow_dir = chat.config.codex_home.join("workflows");
    chat.config.workflows.workflow_dirs = vec![workflow_dir.clone()];
    write_rich_workflow_definition(workflow_dir.join("release.js").as_path());
    chat.config.workflows.named.insert(
        "release".to_string(),
        codex_config::types::WorkflowDefinitionConfig {
            enabled: Some(false),
            approval: None,
        },
    );

    submit_composer_text(&mut chat, "/workflows run release");

    let rendered_history = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered_history.contains("Workflow `release` is disabled by workflow config."),
        "{rendered_history}"
    );
}

#[tokio::test]
async fn plugin_mentions_refresh_updates_workflow_slash_commands() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    let plugin_dir = chat.config.codex_home.join("plugin-workflows");
    write_workflow_definition(
        plugin_dir.join("release.js").as_path(),
        "release",
        "Plugin release",
    );

    chat.on_plugin_mentions_loaded(
        None,
        vec![crate::legacy_core::config::WorkflowPluginDirectory {
            namespace: "sample".to_string(),
            plugin_id: "sample@test".to_string(),
            dir: plugin_dir,
        }],
    );

    submit_composer_text(&mut chat, "/sample:release ship alpha");

    let rendered_history = drain_insert_history(&mut rx)
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        !rendered_history.contains("Unrecognized command"),
        "{rendered_history}"
    );
    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text { text, .. } = &items[0] else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(
                text.contains("Run the saved workflow `sample:release`"),
                "{text}"
            );
            assert!(
                text.contains("workflow tool with name `sample:release`"),
                "{text}"
            );
            assert!(text.contains("ship alpha"), "{text}");
        }
        other => {
            panic!("expected plugin workflow slash command to submit user turn, got {other:?}")
        }
    }
}

#[tokio::test]
async fn slash_workflows_lists_recent_run_snapshots() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_old.json",
        serde_json::json!({
            "run_id": "wf_old",
            "workflow_name": "docs",
            "description": "Update docs",
            "status": "completed",
            "ended_unix_ms": 10_u64,
            "output_preview": "Script completed\nok"
        }),
    );
    write_workflow_run_snapshot(
        &chat,
        "wf_new.json",
        serde_json::json!({
            "run_id": "wf_new",
            "workflow_name": "release",
            "description": "Release workflow",
            "status": "failed",
            "cell_id": "cell-7",
            "source": { "kind": "named", "name": "release", "path": "/tmp/release/source.js" },
            "script_path": "/tmp/release/workflow.js",
            "transcript_dir": "/tmp/release/transcripts",
            "resume_from_run_id": "wf_previous",
            "ended_unix_ms": 20_u64,
            "progress": [
                {
                    "event": "parallel_failed",
                    "unix_ms": 22_u64,
                    "workflow": "release",
                    "message": "item 2: bad branch",
                    "data": {
                        "itemIndex": 2_u64,
                        "error": "bad branch"
                    }
                }
            ],
            "error": "Script failed\nboom"
        }),
    );
    let runs_dir = chat.config.codex_home.join("workflow-runs").to_path_buf();
    std::fs::write(runs_dir.join("broken.json"), "{").expect("write invalid snapshot");

    chat.dispatch_command(SlashCommand::Workflows);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Approval: auto"), "{rendered}");
    assert!(rendered.contains("Recent runs:"), "{rendered}");
    assert!(
        rendered.contains("- failed release `wf_new` (cell `cell-7`)"),
        "{rendered}"
    );
    assert!(rendered.contains("source: named `release`"), "{rendered}");
    assert!(
        rendered.contains("script: `/tmp/release/workflow.js`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("transcripts: `/tmp/release/transcripts`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("resumed from: `wf_previous`"),
        "{rendered}"
    );
    assert!(
        rendered
            .contains("progress: parallel_failed workflow `release` item 2 - item 2: bad branch"),
        "{rendered}"
    );
    assert!(rendered.contains("Script failed / boom"), "{rendered}");
    assert!(rendered.contains("- completed docs `wf_old`"), "{rendered}");
    assert!(
        rendered.contains("Skipped invalid run snapshots: 1"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_sorts_runs_by_updated_progress_ended_and_started_times() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_started.json",
        serde_json::json!({
            "run_id": "wf_started",
            "workflow_name": "started",
            "status": "running",
            "startedUnixMs": 70_u64
        }),
    );
    write_workflow_run_snapshot(
        &chat,
        "wf_ended.json",
        serde_json::json!({
            "run_id": "wf_ended",
            "workflow_name": "ended",
            "status": "completed",
            "ended_unix_ms": 80_u64
        }),
    );
    write_workflow_run_snapshot(
        &chat,
        "wf_progress.json",
        serde_json::json!({
            "run_id": "wf_progress",
            "workflow_name": "progress",
            "status": "running",
            "ended_unix_ms": 10_u64,
            "progress": [
                { "event": "phase", "unix_ms": 90_u64, "phase": "scan" }
            ]
        }),
    );
    write_workflow_run_snapshot(
        &chat,
        "wf_updated.json",
        serde_json::json!({
            "run_id": "wf_updated",
            "workflow_name": "updated",
            "status": "running",
            "updatedUnixMs": 100_u64,
            "ended_unix_ms": 10_u64
        }),
    );

    chat.dispatch_command(SlashCommand::Workflows);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    let updated = rendered.find("`wf_updated`").expect(&rendered);
    let progress = rendered.find("`wf_progress`").expect(&rendered);
    let ended = rendered.find("`wf_ended`").expect(&rendered);
    let started = rendered.find("`wf_started`").expect(&rendered);
    assert!(updated < progress, "{rendered}");
    assert!(progress < ended, "{rendered}");
    assert!(ended < started, "{rendered}");
}

#[tokio::test]
async fn slash_workflows_loads_transcript_run_json_when_index_missing() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let runs_dir = chat.config.codex_home.join("workflow-runs");
    let imported_run_dir = runs_dir.join("wf_transcript_only");
    let imported_transcript_dir = imported_run_dir.join("transcripts");
    let imported_script_path = imported_run_dir.join("script.js");
    write_workflow_transcript_snapshot(
        &chat,
        "wf_transcript_only",
        serde_json::json!({
            "run_id": "wf_transcript_only",
            "workflow_name": "imported",
            "description": "Recovered from transcript metadata",
            "status": "completed",
            "script_path": imported_script_path.display().to_string(),
            "ended_unix_ms": 30_u64,
            "output_preview": "Script completed\nimport-ok",
            "workflowProgress": [
                {
                    "type": "workflow_agent",
                    "index": 1_u64,
                    "label": "Import review",
                    "agentId": "/root/workflow_import_1",
                    "state": "done",
                    "lastProgressAt": 25_u64,
                    "resultPreview": "import reviewed"
                }
            ]
        }),
    );
    std::fs::write(
        imported_transcript_dir.join("agent-root_workflow_import_1.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","isSidechain":true,"message":{"content":[{"type":"text","text":"inspect imported transcript"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":true,"message":{"content":[{"type":"text","text":"import transcript final"}]}}"#,
            "\n"
        ),
    )
    .expect("write imported agent transcript");
    std::fs::write(
        imported_transcript_dir.join("agent-root_workflow_import_1.meta.json"),
        serde_json::json!({
            "version": "codex-workflow-agent-meta-v1",
            "agentId": "/root/workflow_import_1",
            "taskName": "/root/workflow_import_1",
            "agentType": "explorer",
            "runId": "wf_transcript_only",
            "cwd": "/tmp/imported"
        })
        .to_string(),
    )
    .expect("write imported agent metadata");
    write_workflow_run_snapshot(
        &chat,
        "wf_indexed.json",
        serde_json::json!({
            "run_id": "wf_indexed",
            "workflow_name": "indexed",
            "status": "completed",
            "ended_unix_ms": 40_u64,
            "output_preview": "Script completed\nindex-ok"
        }),
    );
    write_workflow_transcript_snapshot(
        &chat,
        "wf_indexed",
        serde_json::json!({
            "run_id": "wf_indexed",
            "workflow_name": "transcript-shadow",
            "status": "completed",
            "ended_unix_ms": 100_u64,
            "output_preview": "should not render"
        }),
    );

    chat.dispatch_command(SlashCommand::Workflows);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("- completed imported `wf_transcript_only`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Recovered from transcript metadata"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("script: `{}`", imported_script_path.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!(
            "transcripts: `{}`",
            imported_transcript_dir.display()
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains("Script completed / import-ok"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- completed indexed `wf_indexed`"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("- completed indexed `wf_indexed`").count(),
        1,
        "{rendered}"
    );
    assert!(!rendered.contains("transcript-shadow"), "{rendered}");
    assert!(!rendered.contains("should not render"), "{rendered}");

    submit_composer_text(&mut chat, "/workflows wf_transcript_only");
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Workflow run `wf_transcript_only`"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("Run dir: `{}`", imported_run_dir.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("Script: `{}`", imported_script_path.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!(
            "Transcripts: `{}`",
            imported_transcript_dir.display()
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains("Script completed\nimport-ok"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript prompt: inspect imported transcript"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  metadata: task `/root/workflow_import_1`; type `explorer`; run `wf_transcript_only`; cwd `/tmp/imported`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript final: import transcript final"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_loads_claude_native_session_layout() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let runs_dir = chat.config.codex_home.join("workflow-runs");
    let session_dir = runs_dir.join("claude-session-import");
    let workflows_dir = session_dir.join("workflows");
    let sidechain_dir = session_dir
        .join("subagents")
        .join("workflows")
        .join("wf_claude_native");
    let script_path = session_dir.join("workflows").join("native-release.js");
    std::fs::create_dir_all(&workflows_dir).expect("create claude workflow dir");
    std::fs::create_dir_all(&sidechain_dir).expect("create claude sidechain dir");
    std::fs::write(
        workflows_dir.join("wf_claude_native.json"),
        serde_json::json!({
            "runId": "wf_claude_native",
            "workflowName": "native-import",
            "description": "Claude native split layout",
            "status": "completed",
            "scriptPath": script_path.display().to_string(),
            "endedUnixMs": 60_u64,
            "workflowProgress": [
                {
                    "type": "workflow_agent",
                    "index": 1_u64,
                    "label": "Native review",
                    "agentId": "/root/workflow_native_1",
                    "state": "done",
                    "lastProgressAt": 55_u64,
                    "resultPreview": "native reviewed"
                }
            ],
            "outputPreview": "Script completed\nnative-ok"
        })
        .to_string(),
    )
    .expect("write claude native workflow snapshot");
    std::fs::write(
        sidechain_dir.join("agent-root_workflow_native_1.jsonl"),
        concat!(
            r#"{"type":"user","uuid":"u1","isSidechain":true,"message":{"content":[{"type":"text","text":"inspect native claude layout"}]}}"#,
            "\n",
            r#"{"type":"assistant","uuid":"a1","parentUuid":"u1","isSidechain":true,"message":{"content":[{"type":"text","text":"native transcript final"}]}}"#,
            "\n"
        ),
    )
    .expect("write claude native sidechain transcript");
    std::fs::write(
        sidechain_dir.join("agent-root_workflow_native_1.meta.json"),
        serde_json::json!({
            "agentId": "/root/workflow_native_1",
            "taskName": "/root/workflow_native_1",
            "agentType": "explorer",
            "runId": "wf_claude_native",
            "cwd": "/tmp/native-import"
        })
        .to_string(),
    )
    .expect("write claude native sidechain metadata");

    chat.dispatch_command(SlashCommand::Workflows);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("- completed native-import `wf_claude_native`"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("script: `{}`", script_path.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("transcripts: `{}`", sidechain_dir.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains("actions: detail `/workflows wf_claude_native`; resume `/workflows resume wf_claude_native`; retry `/workflows retry wf_claude_native`; save `/workflows save wf_claude_native <name>`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Script completed / native-ok"),
        "{rendered}"
    );

    submit_composer_text(&mut chat, "/workflows wf_claude_native");
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Workflow run `wf_claude_native`"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("Run dir: `{}`", sidechain_dir.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("Transcripts: `{}`", sidechain_dir.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains("Actions: detail `/workflows wf_claude_native`; resume `/workflows resume wf_claude_native`; retry `/workflows retry wf_claude_native`; save `/workflows save wf_claude_native <name>`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  metadata: task `/root/workflow_native_1`; type `explorer`; run `wf_claude_native`; cwd `/tmp/native-import`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript prompt: inspect native claude layout"),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript final: native transcript final"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_accepts_camel_case_snapshot_fields() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_camel.json",
        serde_json::json!({
            "runId": "wf_camel",
            "workflowName": "camel",
            "metadataName": "camel-meta",
            "description": "Camel snapshot",
            "inputSchema": "{ type: 'object' }",
            "status": "completed",
            "cellId": "cell-camel",
            "sourceKind": "script_path",
            "sourceName": "camel-source",
            "sourcePath": "/tmp/workflows/camel-source.js",
            "runDir": "/tmp/workflows/wf_camel",
            "scriptPath": "/tmp/workflows/wf_camel/script.js",
            "transcriptDir": "/tmp/workflows/wf_camel/transcripts",
            "resumeFromRunId": "wf_prior",
            "scriptHash": "fnv1a64:camel",
            "endedUnixMs": 50_u64,
            "durationMs": 12_u64,
            "statusHistory": [
                { "event": "started", "unixMs": 40_u64 },
                { "event": "completed", "status": "completed", "unixMs": 50_u64, "message": "done" }
            ],
            "progress": [
                {
                    "event": "phase",
                    "unixMs": 45_u64,
                    "workflow": "camel",
                    "phase": "scan",
                    "message": "scanning"
                }
            ],
            "outputPreview": "Script completed\ncamel-ok"
        }),
    );

    chat.dispatch_command(SlashCommand::Workflows);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("- completed camel `wf_camel` (cell `cell-camel`)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("script: `/tmp/workflows/wf_camel/script.js`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("source: script-path `camel-source`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("transcripts: `/tmp/workflows/wf_camel/transcripts`"),
        "{rendered}"
    );
    assert!(rendered.contains("resumed from: `wf_prior`"), "{rendered}");
    assert!(
        rendered.contains("progress: phase workflow `camel` phase `scan` - scanning"),
        "{rendered}"
    );

    submit_composer_text(&mut chat, "/workflows wf_camel");
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Workflow run `wf_camel`"), "{rendered}");
    assert!(rendered.contains("Metadata: `camel-meta`"), "{rendered}");
    assert!(rendered.contains("Input schema:"), "{rendered}");
    assert!(
        rendered.contains("Run dir: `/tmp/workflows/wf_camel`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Script hash: `fnv1a64:camel`"),
        "{rendered}"
    );
    assert!(rendered.contains("Duration: 12 ms"), "{rendered}");
    assert!(
        rendered.contains("Source: script-path `camel-source`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Source path: `/tmp/workflows/camel-source.js`"),
        "{rendered}"
    );
    assert!(rendered.contains("- completed at 50 - done"), "{rendered}");
    assert!(
        rendered.contains("- phase workflow `camel` phase `scan` - scanning at 45"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_renders_claude_shaped_workflow_progress() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let run_dir = chat.config.codex_home.join("claude-progress-run");
    let transcript_dir = run_dir.join("transcripts");
    std::fs::create_dir_all(&transcript_dir).expect("create claude agent transcript dir");
    std::fs::write(
        transcript_dir.join("agent-root_workflow_release_1.jsonl"),
        concat!(
            r#"{"type":"system","uuid":"s1","isSidechain":true,"message":{"content":"ignored system row"}}"#,
            "\n",
            r#"{"type":"user","uuid":"u1","isSidechain":true,"sessionId":"wf_claude_progress","agentId":"/root/workflow_release_1","cwd":"/tmp/workflows","version":"test","entrypoint":"workflow","message":{"content":[{"type":"text","text":"full compile docs prompt"}]}}"#,
            "\n",
            r#"{"type":"permission-mode","uuid":"p1","isSidechain":true,"mode":"default"}"#,
            "\n",
            r##"{"type":"assistant","uuid":"a1","parentUuid":"u1","logicalParentUuid":"u1","isSidechain":true,"sessionId":"wf_claude_progress","agentId":"/root/workflow_release_1","cwd":"/tmp/workflows","version":"test","entrypoint":"workflow","message":{"content":[{"type":"reasoning","summary":[{"type":"summary_text","text":"planned docs outline"}]},{"type":"tool_use","id":"toolu_glob_docs","name":"Glob","input":{"pattern":"docs/**/*.md"}}]}}"##,
            "\n",
            r##"{"type":"assistant","uuid":"a2","parentUuid":"a1","isSidechain":true,"sessionId":"wf_claude_progress","agentId":"/root/workflow_release_1","message":{"content":[{"type":"reasoning","summary":[{"type":"summary_text","text":"checked docs structure"}]},{"type":"tool_use","id":"toolu_read_docs","name":"Read","input":{"file_path":"docs.md"}}]}}"##,
            "\n",
            r##"{"type":"user","uuid":"u2","parentUuid":"a2","isSidechain":true,"sessionId":"wf_claude_progress","agentId":"/root/workflow_release_1","sourceToolAssistantUUID":"a1","toolUseResult":"docs.md"}"##,
            "\n",
            r##"{"type":"tool_result","uuid":"t2","parentUuid":"a2","isSidechain":true,"sessionId":"wf_claude_progress","agentId":"/root/workflow_release_1","tool_use_id":"toolu_read_docs","content":[{"type":"text","text":"# Docs"}]}"##,
            "\n",
            r##"{"type":"assistant","uuid":"a3","parentUuid":"t2","isSidechain":true,"sessionId":"wf_claude_progress","agentId":"/root/workflow_release_1","message":{"content":[{"type":"text","text":"full docs built final"}]}}"##,
            "\n"
        ),
    )
    .expect("write claude agent transcript");
    std::fs::write(
        transcript_dir.join("agent-root_workflow_release_1.meta.json"),
        serde_json::json!({
            "version": "codex-workflow-agent-meta-v1",
            "agentId": "/root/workflow_release_1",
            "taskName": "/root/workflow_release_1",
            "agentName": "Build docs",
            "sessionKind": "workflow_agent",
            "parentThreadId": "thread-parent",
            "agentType": "explorer",
            "model": "gpt-5.5",
            "reasoningEffort": "xhigh",
            "serviceTier": "priority",
            "nickname": "Ada",
            "toolUseId": "toolu_spawn_docs",
            "runId": "wf_claude_progress",
            "cellId": "cell-claude",
            "cwd": "/tmp/workflows",
            "gitBranch": "feature/workflows",
            "worktreePath": "/tmp/workflows/.codex/worktrees/docs",
            "author": "/root/workflow_release_1",
            "recipient": "/root"
        })
        .to_string(),
    )
    .expect("write claude agent metadata");
    write_workflow_run_snapshot(
        &chat,
        "wf_claude_progress.json",
        serde_json::json!({
            "runId": "wf_claude_progress",
            "workflowName": "imported",
            "description": "Claude-shaped progress",
            "status": "running",
            "cellId": "cell-claude",
            "runDir": run_dir.display().to_string(),
            "workflowProgress": [
                {
                    "type": "workflow_phase",
                    "index": 1_u64,
                    "title": "Build",
                    "lastProgressAt": 100_u64
                },
                {
                    "type": "workflow_agent",
                    "index": 1_u64,
                    "label": "Build docs",
                    "phaseTitle": "Build",
                    "agentId": "/root/workflow_release_1",
                    "state": "start",
                    "lastProgressAt": 110_u64,
                    "promptPreview": "compile docs"
                },
                {
                    "type": "workflow_agent",
                    "index": 1_u64,
                    "label": "Build docs",
                    "phaseTitle": "Build",
                    "agentId": "/root/workflow_release_1",
                    "state": "done",
                    "lastProgressAt": 140_u64,
                    "resultPreview": "docs built"
                },
                {
                    "type": "workflow_agent",
                    "index": 2_u64,
                    "label": "Lint docs",
                    "phaseTitle": "Build",
                    "agentId": "agent-2",
                    "state": "error",
                    "lastProgressAt": 150_u64,
                    "promptPreview": "lint docs",
                    "error": "lint failed"
                },
                {
                    "type": "workflow_log",
                    "message": "workflow imported",
                    "lastProgressAt": 160_u64
                }
            ]
        }),
    );

    chat.dispatch_command(SlashCommand::Workflows);
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("- running imported `wf_claude_progress` (cell `cell-claude`)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("progress: workflow_log - workflow imported"),
        "{rendered}"
    );

    submit_composer_text(&mut chat, "/workflows wf_claude_progress");
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Summary:"), "{rendered}");
    assert!(rendered.contains("Phases (1):"), "{rendered}");
    assert!(rendered.contains("- Build: reached at 100"), "{rendered}");
    assert!(rendered.contains("Agents (2):"), "{rendered}");
    assert!(
        rendered.contains("- Build docs: completed at 140 - docs built"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- Lint docs: failed at 150 - lint docs"),
        "{rendered}"
    );
    assert!(rendered.contains("Agent details (2):"), "{rendered}");
    assert!(
        rendered.contains("- Build docs (#1, /root/workflow_release_1): completed at 140"),
        "{rendered}"
    );
    assert!(rendered.contains("  prompt: compile docs"), "{rendered}");
    assert!(rendered.contains("  result: docs built"), "{rendered}");
    assert!(
        rendered.contains("  transcript prompt: full compile docs prompt"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "  metadata: task `/root/workflow_release_1`; agent `Build docs`; session `workflow_agent`; parent thread `thread-parent`; type `explorer`; model `gpt-5.5`; effort `xhigh`; tier `priority`; nick `Ada`; tool `toolu_spawn_docs`; run `wf_claude_progress`; cell `cell-claude`; cwd `/tmp/workflows`; branch `feature/workflows`; worktree `/tmp/workflows/.codex/worktrees/docs`; author `/root/workflow_release_1`; recipient `/root`"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript reasoning: planned docs outline; checked docs structure"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            r#"  activity: Glob {"pattern":"docs/**/*.md"} => docs.md; Read {"file_path":"docs.md"} => # Docs"#
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript final: full docs built final"),
        "{rendered}"
    );
    assert!(!rendered.contains("transcript skipped:"), "{rendered}");
    assert!(
        rendered.contains("- Lint docs (#2, agent-2): failed at 150"),
        "{rendered}"
    );
    assert!(rendered.contains("  error: lint failed"), "{rendered}");
    assert!(
        rendered.contains(
            "- workflow_agent #2 phase `Build` agent `Lint docs` state `error` - lint docs (error: lint failed) at 150"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("- workflow_log - workflow imported at 160"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_detail_renders_raw_agent_transcript_notification_records() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let run_dir = chat.config.codex_home.join("raw-agent-transcript-run");
    let transcript_dir = run_dir.join("transcripts");
    std::fs::create_dir_all(&transcript_dir).expect("create raw agent transcript dir");
    std::fs::write(
        transcript_dir.join("agent-root_workflow_release_1.jsonl"),
        concat!(
            r##"{"role":"assistant","content":[{"type":"reasoning","summary":[{"type":"summary_text","text":"planned raw child history"}]}]}"##,
            "\n",
            r##"{"role":"assistant","content":[{"type":"tool_use","id":"toolu_glob","name":"Glob","input":{"pattern":"src/**/*.rs"}}]}"##,
            "\n",
            r##"{"type":"tool_result","tool_use_id":"toolu_glob","content":"src/lib.rs"}"##,
            "\n",
            r##"{"role":"assistant","content":[{"type":"reasoning","summary":[{"type":"summary_text","text":"read raw child history"}]}]}"##,
            "\n",
            r##"{"role":"assistant","content":[{"type":"tool_use","id":"toolu_read","name":"Read","input":{"file_path":"src/lib.rs"}}]}"##,
            "\n",
            r##"{"type":"tool_result","tool_use_id":"toolu_read","content":"pub fn ok() {}"}"##,
            "\n",
            r##"{"role":"assistant","content":[{"type":"text","text":"review complete"}]}"##,
            "\n"
        ),
    )
    .expect("write raw agent transcript");
    write_workflow_run_snapshot(
        &chat,
        "wf_raw_transcript.json",
        serde_json::json!({
            "runId": "wf_raw_transcript",
            "workflowName": "release",
            "description": "Raw agent transcript",
            "status": "running",
            "cellId": "cell-raw",
            "runDir": run_dir.display().to_string(),
            "workflowProgress": [
                {
                    "type": "workflow_agent",
                    "index": 1_u64,
                    "label": "Review",
                    "phaseTitle": "Build",
                    "agentId": "/root/workflow_release_1",
                    "state": "done",
                    "lastProgressAt": 140_u64,
                    "resultPreview": "review summarized"
                }
            ]
        }),
    );

    submit_composer_text(&mut chat, "/workflows wf_raw_transcript");

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered
            .contains("  transcript reasoning: planned raw child history; read raw child history"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            r#"  activity: Glob {"pattern":"src/**/*.rs"} => src/lib.rs; Read {"file_path":"src/lib.rs"} => pub fn ok() {}"#
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("  transcript final: review complete"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_lists_active_run_registry_without_recent_duplicate() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let running = serde_json::json!({
        "run_id": "wf_running",
        "workflow_name": "release",
        "description": "Release workflow",
        "status": "running",
        "cell_id": "cell-42",
        "transcript_dir": "/tmp/workflows/wf_running/transcripts",
        "ended_unix_ms": 30_u64,
        "progress": [
            {
                "event": "phase",
                "unix_ms": 40_u64,
                "workflow": "release",
                "phase": "publish",
                "message": "publishing"
            }
        ]
    });
    write_workflow_run_snapshot(&chat, "wf_running.json", running.clone());
    write_active_workflow_run_marker(&chat, "wf_running.json", running);
    write_workflow_run_snapshot(
        &chat,
        "wf_done.json",
        serde_json::json!({
            "run_id": "wf_done",
            "workflow_name": "docs",
            "description": "Docs workflow",
            "status": "completed",
            "ended_unix_ms": 20_u64,
            "output_preview": "Script completed\nok"
        }),
    );

    chat.dispatch_command(SlashCommand::Workflows);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Active registry:"), "{rendered}");
    assert!(rendered.contains("Active runs:"), "{rendered}");
    assert!(
        rendered.contains("- running release `wf_running` (cell `cell-42`)"),
        "{rendered}"
    );
    assert!(
        rendered.contains("transcripts: `/tmp/workflows/wf_running/transcripts`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("progress: phase workflow `release` phase `publish` - publishing"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "actions: detail `/workflows wf_running`; pause `/workflows pause wf_running`; cancel `/workflows cancel wf_running`"
        ),
        "{rendered}"
    );
    assert!(rendered.contains("Recent runs:"), "{rendered}");
    assert!(
        rendered.contains("- completed docs `wf_done`"),
        "{rendered}"
    );
    assert_eq!(
        rendered.matches("- running release `wf_running`").count(),
        1,
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_action_summaries_include_running_agent_controls() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_agent_controls.json",
        serde_json::json!({
        "run_id": "wf_agent_controls",
        "workflow_name": "release",
        "description": "Release workflow",
        "status": "running",
        "cell_id": "cell-123",
        "progress": [
                {
                    "event": "workflow_agent",
                    "state": "running",
                    "agent": "build_agent",
                    "agentId": "/root/workflow_release_1",
                    "message": "build artifacts"
                },
                {
                    "event": "agent_waiting",
                    "agent": "build_agent",
                    "agentId": "/root/workflow_release_1",
                    "message": "no agent update for 60s",
                    "data": { "elapsedMs": 60000, "timeoutMs": 180000 }
                }
            ]
        }),
    );

    let expected_actions = concat!(
        "detail `/workflows wf_agent_controls`; ",
        "pause `/workflows pause wf_agent_controls`; ",
        "cancel `/workflows cancel wf_agent_controls`; ",
        "interrupt-agent `/workflows interrupt-agent wf_agent_controls /root/workflow_release_1`; ",
        "skip-agent `/workflows skip-agent wf_agent_controls /root/workflow_release_1`; ",
        "retry-agent `/workflows retry-agent wf_agent_controls /root/workflow_release_1`; ",
        "restart-agent `/workflows restart-agent wf_agent_controls /root/workflow_release_1`"
    );

    chat.dispatch_command(SlashCommand::Workflows);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(&format!("actions: {expected_actions}")),
        "{rendered}"
    );

    submit_composer_text(&mut chat, "/workflows wf_agent_controls");

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(&format!("Actions: {expected_actions}")),
        "{rendered}"
    );
    assert!(
        rendered.contains("- build_agent (/root/workflow_release_1): running"),
        "{rendered}"
    );
    assert!(rendered.contains("no agent update for 60s"), "{rendered}");
}

#[tokio::test]
async fn slash_workflows_run_id_shows_run_detail() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let run_dir = chat
        .config
        .codex_home
        .join("workflow-runs")
        .join("wf_detail");
    std::fs::create_dir_all(&run_dir).expect("create run dir");
    std::fs::write(
        run_dir.join("journal.jsonl"),
        concat!(
            r#"{"type":"started","key":"codex-v2:first","agentId":"agent-one"}"#,
            "\n",
            r#"{"type":"started","key":"codex-v2:first","agentId":"agent-two"}"#,
            "\n",
            r#"{"type":"result","key":"codex-v2:first","agentId":"agent-two","result":"ok"}"#,
            "\n",
            r#"{"type":"child_result","key":"codex-child-v1:first","child":"smoke","childRunId":"smoke#1","result":"smoke-ok"}"#,
            "\n",
        ),
    )
    .expect("write journal");
    write_workflow_run_snapshot(
        &chat,
        "wf_detail.json",
        serde_json::json!({
            "run_id": "wf_detail",
            "workflow_name": "release",
            "metadata_name": "release-meta",
            "description": "Release workflow",
            "input_schema": "{ type: 'object', properties: { channel: { type: 'string' } } }",
            "status": "completed",
            "cell_id": "cell-9",
            "session_id": "session-9",
            "thread_id": "thread-9",
            "workflow_tool_call_id": "toolu_workflow_9",
            "cwd": "/tmp/project",
            "git_branch": "feature/workflows",
            "run_dir": run_dir.display().to_string(),
            "script_path": run_dir.join("script.js").display().to_string(),
            "transcript_dir": run_dir.join("transcripts").display().to_string(),
            "script_hash": "fnv1a64:abcdef1234567890",
            "source": {
                "kind": "named",
                "name": "release",
                "path": "/tmp/workflows/release.js"
            },
            "resume_from_run_id": "wf_previous",
            "max_output_tokens": 2048,
            "duration_ms": 42_u64,
            "args": { "channel": "alpha" },
            "status_history": [
                { "event": "started", "unix_ms": 100_u64 },
                { "event": "running", "status": "running", "unix_ms": 120_u64 },
                { "event": "completed", "status": "completed", "unix_ms": 142_u64, "message": "Script completed\nok" }
            ],
            "progress": [
                { "event": "workflow_start", "unix_ms": 101_u64, "workflow": "release", "message": "Release workflow" },
                { "event": "phase", "unix_ms": 110_u64, "workflow": "release", "phase": "build", "message": "artifacts" },
                { "event": "agent_start", "unix_ms": 120_u64, "workflow": "release", "agent": "build_agent", "message": "build artifacts" },
                { "event": "agent_complete", "unix_ms": 130_u64, "workflow": "release", "agent": "build_agent" },
                {
                    "event": "child_complete",
                    "unix_ms": 136_u64,
                    "workflow": "release",
                    "child": "smoke",
                    "child_index": 1_u64,
                    "child_run_id": "smoke#1"
                },
                {
                    "event": "child_failed",
                    "unix_ms": 138_u64,
                    "workflow": "release",
                    "child": "smoke",
                    "message": "boom",
                    "data": {
                        "childIndex": 2_u64,
                        "childRunId": "smoke#2",
                        "error": "boom"
                    }
                },
                {
                    "event": "pipeline_failed",
                    "unix_ms": 140_u64,
                    "workflow": "release",
                    "message": "item 3 stage 1: stage failed",
                    "data": {
                        "itemIndex": 3_u64,
                        "stageIndex": 1_u64,
                        "error": "stage failed"
                    }
                },
                {
                    "event": "workflow_complete",
                    "unix_ms": 142_u64,
                    "workflow": "release",
                    "data": {
                        "agentCount": 1_u64,
                        "childCount": 2_u64,
                        "logCount": 42_u64,
                        "logSuppressed": true
                    }
                }
            ],
            "output_preview": "Script completed\nok"
        }),
    );

    submit_composer_text(&mut chat, "/workflows wf_detail");

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflows detail message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Workflow run `wf_detail`"), "{rendered}");
    assert!(rendered.contains("Status: completed"), "{rendered}");
    assert!(rendered.contains("Metadata: `release-meta`"), "{rendered}");
    assert!(rendered.contains("Input schema:"), "{rendered}");
    assert!(
        rendered.contains("{ type: 'object', properties: { channel: { type: 'string' } } }"),
        "{rendered}"
    );
    assert!(rendered.contains("Cell: `cell-9`"), "{rendered}");
    assert!(rendered.contains("Session: `session-9`"), "{rendered}");
    assert!(rendered.contains("Thread: `thread-9`"), "{rendered}");
    assert!(
        rendered.contains("Workflow tool: `toolu_workflow_9`"),
        "{rendered}"
    );
    assert!(rendered.contains("Cwd: `/tmp/project`"), "{rendered}");
    assert!(
        rendered.contains("Branch: `feature/workflows`"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!("Run dir: `{}`", run_dir.display())),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!(
            "Agent journal: `{}`",
            run_dir.join("journal.jsonl").display()
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains("Journal entries: 2 started, 1 result, 1 child result"),
        "{rendered}"
    );
    assert!(rendered.contains("Journal agents (2):"), "{rendered}");
    assert!(
        rendered.contains("- agent-one: started `codex-v2:first`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- agent-two: completed `codex-v2:first`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Journal child workflows (1):"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- smoke#1: completed `smoke` `codex-child-v1:first`"),
        "{rendered}"
    );
    assert!(rendered.contains("Source: named `release`"), "{rendered}");
    assert!(
        rendered.contains("Source path: `/tmp/workflows/release.js`"),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!(
            "Script: `{}`",
            run_dir.join("script.js").display()
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains(&format!(
            "Transcripts: `{}`",
            run_dir.join("transcripts").display()
        )),
        "{rendered}"
    );
    assert!(
        rendered.contains("Script hash: `fnv1a64:abcdef1234567890`"),
        "{rendered}"
    );
    assert!(rendered.contains("Max output tokens: 2048"), "{rendered}");
    assert!(
        rendered.contains("Resumed from: `wf_previous`"),
        "{rendered}"
    );
    assert!(rendered.contains("\"channel\": \"alpha\""), "{rendered}");
    assert!(
        rendered.contains(
            "Actions: detail `/workflows wf_detail`; resume `/workflows resume wf_detail`; retry `/workflows retry wf_detail`; save `/workflows save wf_detail <name>`"
        ),
        "{rendered}"
    );
    assert!(
        rendered
            .contains("Run metrics: 1 agent; 2 child workflows; 42 logs (suppressed); 2 failures"),
        "{rendered}"
    );
    assert!(rendered.contains("Summary:"), "{rendered}");
    assert!(rendered.contains("Phases (1):"), "{rendered}");
    assert!(
        rendered.contains("- build: reached at 110 - artifacts"),
        "{rendered}"
    );
    assert!(rendered.contains("Agents (1):"), "{rendered}");
    assert!(
        rendered.contains("- build_agent: completed at 130"),
        "{rendered}"
    );
    assert!(rendered.contains("Child workflows (2):"), "{rendered}");
    assert!(
        rendered.contains("- smoke#1: completed at 136"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- smoke#2: failed at 138 - boom"),
        "{rendered}"
    );
    assert!(rendered.contains("History:"), "{rendered}");
    assert!(rendered.contains("- started at 100"), "{rendered}");
    assert!(rendered.contains("- running at 120"), "{rendered}");
    assert!(
        rendered.contains("- completed at 142 - Script completed / ok"),
        "{rendered}"
    );
    assert!(rendered.contains("Progress:"), "{rendered}");
    assert!(
        rendered.contains("- workflow_start workflow `release` - Release workflow at 101"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- phase workflow `release` phase `build` - artifacts at 110"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "- agent_start workflow `release` agent `build_agent` - build artifacts at 120"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("- agent_complete workflow `release` agent `build_agent` at 130"),
        "{rendered}"
    );
    assert!(
        rendered.contains("- child_complete workflow `release` child `smoke` run `smoke#1` at 136"),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "- child_failed workflow `release` child `smoke` run `smoke#2` - boom at 138"
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains(
            "- pipeline_failed workflow `release` item 3 stage 1 - item 3 stage 1: stage failed at 140"
        ),
        "{rendered}"
    );
    assert!(rendered.contains("Script completed"), "{rendered}");
}

#[tokio::test]
async fn slash_workflows_resume_submits_resume_invocation_prompt() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_resume.json",
        serde_json::json!({
            "run_id": "wf_resume",
            "workflow_name": "release",
            "status": "completed",
            "script_path": "/tmp/workflows/wf_resume/script.js",
            "script_hash": "fnv1a64:abcdef1234567890",
            "args": { "channel": "alpha" },
            "output_preview": "Script completed\nok"
        }),
    );

    submit_composer_text(&mut chat, "/workflows resume wf_resume");

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text {
                text,
                text_elements,
            } = &items[0]
            else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(text.contains("Resume workflow run `wf_resume`"), "{text}");
            assert!(text.contains("resumeFromRunId: \"wf_resume\""), "{text}");
            assert!(
                text.contains("Do not run the workflow script manually"),
                "{text}"
            );
            assert!(
                text.contains("Prior script path: /tmp/workflows/wf_resume/script.js"),
                "{text}"
            );
            assert!(
                text.contains("Prior script hash: fnv1a64:abcdef1234567890"),
                "{text}"
            );
            assert!(text.contains("\"channel\": \"alpha\""), "{text}");
            assert!(text_elements.is_empty(), "{text_elements:?}");
        }
        other => panic!("expected workflow resume to submit user turn, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_workflows_retry_submits_retry_invocation_prompt() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_retry.json",
        serde_json::json!({
            "run_id": "wf_retry",
            "workflow_name": "release",
            "status": "failed",
            "script_path": "/tmp/workflows/wf_retry/script.js",
            "script_hash": "fnv1a64:feedface12345678",
            "args": { "channel": "alpha" },
            "error": "Script failed\nboom"
        }),
    );

    submit_composer_text(&mut chat, "/workflows retry wf_retry");

    match next_submit_op(&mut op_rx) {
        Op::UserTurn { items, .. } => {
            assert_eq!(items.len(), 1, "{items:?}");
            let UserInput::Text {
                text,
                text_elements,
            } = &items[0]
            else {
                panic!("expected text user input, got {items:?}");
            };
            assert!(text.contains("Retry workflow run `wf_retry`"), "{text}");
            assert!(text.contains("workflow tool"), "{text}");
            assert!(
                text.contains("scriptPath: \"/tmp/workflows/wf_retry/script.js\""),
                "{text}"
            );
            assert!(text.contains("Do not use `resumeFromRunId`"), "{text}");
            assert!(
                text.contains("Prior script hash: fnv1a64:feedface12345678"),
                "{text}"
            );
            assert!(text.contains("\"channel\": \"alpha\""), "{text}");
            assert!(text_elements.is_empty(), "{text_elements:?}");
        }
        other => panic!("expected workflow retry to submit user turn, got {other:?}"),
    }
}

#[tokio::test]
async fn slash_workflows_retry_rejects_run_without_script_path() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_no_script.json",
        serde_json::json!({
            "run_id": "wf_no_script",
            "workflow_name": "release",
            "status": "failed"
        }),
    );

    submit_composer_text(&mut chat, "/workflows retry wf_no_script");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Workflow run `wf_no_script` has no script path to retry."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_save_copies_run_script_into_workflow_directory() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let workflow_dir = chat.config.codex_home.join("project-workflows");
    chat.config.workflows.workflow_dirs = vec![workflow_dir.clone()];
    let script_path = chat
        .config
        .codex_home
        .join("workflow-runs")
        .join("wf_save")
        .join("script.js");
    std::fs::create_dir_all(script_path.parent().expect("script parent"))
        .expect("create script parent");
    let script = "export const meta = { name: 'release', description: 'Release workflow' };\nphase('ship');\n";
    std::fs::write(&script_path, script).expect("write run script");
    write_workflow_run_snapshot(
        &chat,
        "wf_save.json",
        serde_json::json!({
            "run_id": "wf_save",
            "workflow_name": "release",
            "status": "completed",
            "script_path": script_path.display().to_string(),
            "output_preview": "ok"
        }),
    );

    submit_composer_text(&mut chat, "/workflows save wf_save saved-release");

    let target_path = workflow_dir.join("saved-release.js");
    let saved = std::fs::read_to_string(&target_path).expect("read saved workflow");
    assert_eq!(saved, script);
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Saved workflow `saved-release`"),
        "{rendered}"
    );
    assert!(
        rendered.contains("Use /workflows run saved-release or /saved-release to run it."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_save_rejects_script_without_metadata() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let workflow_dir = chat.config.codex_home.join("project-workflows");
    chat.config.workflows.workflow_dirs = vec![workflow_dir.clone()];
    let script_path = chat
        .config
        .codex_home
        .join("workflow-runs")
        .join("wf_bad_script")
        .join("script.js");
    std::fs::create_dir_all(script_path.parent().expect("script parent"))
        .expect("create script parent");
    std::fs::write(&script_path, "phase('missing meta');\n").expect("write invalid run script");
    write_workflow_run_snapshot(
        &chat,
        "wf_bad_script.json",
        serde_json::json!({
            "run_id": "wf_bad_script",
            "workflow_name": "release",
            "status": "completed",
            "script_path": script_path.display().to_string()
        }),
    );

    submit_composer_text(&mut chat, "/workflows save wf_bad_script saved-release");

    assert!(!workflow_dir.join("saved-release.js").exists());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("script does not contain a valid `export const meta` header"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_cancel_submits_direct_cancel_for_running_run() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123",
            "transcript_dir": "/tmp/workflows/wf_running/transcripts"
        }),
    );

    submit_composer_text(&mut chat, "/workflows cancel wf_running");

    assert_eq!(
        op_rx.try_recv().expect("expected workflow cancel op"),
        Op::WorkflowCancel {
            run_id: "wf_running".to_string(),
            cell_id: "cell-123".to_string(),
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Cancellation requested for workflow run `wf_running`."),
        "{rendered}"
    );
    assert!(
        rendered.contains("Cell `cell-123` will be terminated directly."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_cancel_submits_direct_cancel_for_paused_run() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_paused.json",
        serde_json::json!({
            "run_id": "wf_paused",
            "workflow_name": "release",
            "status": "paused",
            "cell_id": "cell-123"
        }),
    );

    submit_composer_text(&mut chat, "/workflows cancel wf_paused");

    assert_eq!(
        op_rx.try_recv().expect("expected workflow cancel op"),
        Op::WorkflowCancel {
            run_id: "wf_paused".to_string(),
            cell_id: "cell-123".to_string(),
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Cancellation requested for workflow run `wf_paused`."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_pause_submits_direct_pause_for_running_run() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123"
        }),
    );

    submit_composer_text(&mut chat, "/workflows pause wf_running");

    assert_eq!(
        op_rx.try_recv().expect("expected workflow pause op"),
        Op::WorkflowPause {
            run_id: "wf_running".to_string(),
            cell_id: "cell-123".to_string(),
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Pause requested for workflow run `wf_running`."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_continue_submits_direct_continue_for_paused_run() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_paused.json",
        serde_json::json!({
            "run_id": "wf_paused",
            "workflow_name": "release",
            "status": "paused",
            "cell_id": "cell-123"
        }),
    );

    submit_composer_text(&mut chat, "/workflows continue wf_paused");

    assert_eq!(
        op_rx.try_recv().expect("expected workflow continue op"),
        Op::WorkflowContinue {
            run_id: "wf_paused".to_string(),
            cell_id: "cell-123".to_string(),
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Continue requested for workflow run `wf_paused`."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_interrupt_agent_submits_direct_interrupt_for_running_agent() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123",
            "progress": [
                {
                    "event": "agent_start",
                    "agent": "build_agent",
                    "agent_id": "/root/workflow_release_1",
                    "message": "build artifacts"
                }
            ]
        }),
    );

    submit_composer_text(
        &mut chat,
        "/workflows interrupt-agent wf_running /root/workflow_release_1",
    );

    assert_eq!(
        op_rx
            .try_recv()
            .expect("expected workflow agent interrupt op"),
        Op::WorkflowAgentInterrupt {
            run_id: "wf_running".to_string(),
            agent_id: "/root/workflow_release_1".to_string(),
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(
            "Interrupt requested for workflow run `wf_running` agent `/root/workflow_release_1`."
        ),
        "{rendered}"
    );
    assert!(
        rendered.contains("The agent turn will be interrupted directly."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_skip_agent_submits_selected_agent_control() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123",
            "progress": [
                {
                    "event": "agent_start",
                    "agent": "build_agent",
                    "agent_id": "/root/workflow_release_1",
                    "message": "build artifacts"
                }
            ]
        }),
    );

    submit_composer_text(
        &mut chat,
        "/workflows skip-agent wf_running /root/workflow_release_1",
    );

    assert_eq!(
        op_rx
            .try_recv()
            .expect("expected workflow agent control op"),
        Op::WorkflowAgentControl {
            run_id: "wf_running".to_string(),
            agent_id: "/root/workflow_release_1".to_string(),
            action: codex_protocol::protocol::WorkflowAgentControlAction::Skip,
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(
            "Skip requested for workflow run `wf_running` agent `/root/workflow_release_1`."
        ),
        "{rendered}"
    );
    assert!(
        rendered
            .contains("The workflow runtime will apply the request without cancelling the run."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_retry_agent_submits_selected_agent_control() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123",
            "progress": [
                {
                    "event": "agent_start",
                    "agent": "build_agent",
                    "agent_id": "/root/workflow_release_1"
                }
            ]
        }),
    );

    submit_composer_text(
        &mut chat,
        "/workflows retry-agent wf_running /root/workflow_release_1",
    );

    assert_eq!(
        op_rx
            .try_recv()
            .expect("expected workflow agent control op"),
        Op::WorkflowAgentControl {
            run_id: "wf_running".to_string(),
            agent_id: "/root/workflow_release_1".to_string(),
            action: codex_protocol::protocol::WorkflowAgentControlAction::Retry,
        }
    );
}

#[tokio::test]
async fn slash_workflows_restart_agent_submits_selected_agent_control() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123",
            "progress": [
                {
                    "event": "workflow_agent",
                    "state": "running",
                    "agent": "build_agent",
                    "agentId": "/root/workflow_release_1"
                }
            ]
        }),
    );

    submit_composer_text(
        &mut chat,
        "/workflows restart-agent wf_running /root/workflow_release_1",
    );

    assert_eq!(
        op_rx
            .try_recv()
            .expect("expected workflow agent control op"),
        Op::WorkflowAgentControl {
            run_id: "wf_running".to_string(),
            agent_id: "/root/workflow_release_1".to_string(),
            action: codex_protocol::protocol::WorkflowAgentControlAction::Retry,
        }
    );
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(
            "Restart requested for workflow run `wf_running` agent `/root/workflow_release_1`."
        ),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_interrupt_agent_rejects_non_running_agent() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_running.json",
        serde_json::json!({
            "run_id": "wf_running",
            "workflow_name": "release",
            "status": "running",
            "cell_id": "cell-123",
            "progress": [
                {
                    "event": "agent_start",
                    "agent": "build_agent",
                    "agent_id": "/root/workflow_release_1"
                },
                {
                    "event": "agent_complete",
                    "agent": "build_agent",
                    "agent_id": "/root/workflow_release_1"
                }
            ]
        }),
    );

    submit_composer_text(
        &mut chat,
        "/workflows interrupt-agent wf_running /root/workflow_release_1",
    );

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains(
            "Workflow run `wf_running` agent `/root/workflow_release_1` is `completed` and cannot be interrupted."
        ),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_cancel_rejects_completed_run() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_done.json",
        serde_json::json!({
            "run_id": "wf_done",
            "workflow_name": "release",
            "status": "completed",
            "cell_id": "cell-123"
        }),
    );

    submit_composer_text(&mut chat, "/workflows cancel wf_done");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Workflow run `wf_done` is `completed` and cannot be cancelled."),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_workflows_approval_emits_named_workflow_update() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/workflows approval sample:release allow");

    let mut found = None;
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::UpdateNamedWorkflowApproval {
            workflow_name,
            approval,
        } = event
        {
            found = Some((workflow_name, approval));
            break;
        }
    }
    assert_eq!(
        found,
        Some((
            "sample:release".to_string(),
            Some(codex_config::types::WorkflowApproval::Allow)
        ))
    );
}

#[tokio::test]
async fn slash_workflows_enabled_emits_named_workflow_update() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/workflows enabled sample:release off");
    submit_composer_text(&mut chat, "/workflows enable docs");

    let mut found = Vec::new();
    while let Ok(event) = rx.try_recv() {
        if let AppEvent::UpdateNamedWorkflowEnabled {
            workflow_name,
            enabled,
        } = event
        {
            found.push((workflow_name, enabled));
        }
    }
    assert_eq!(
        found,
        vec![
            ("sample:release".to_string(), Some(false)),
            ("docs".to_string(), Some(true)),
        ]
    );
}

#[tokio::test]
async fn slash_workflow_status_stays_compact_without_run_browser() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    write_workflow_run_snapshot(
        &chat,
        "wf_visible_only_in_plural.json",
        serde_json::json!({
            "run_id": "wf_visible_only_in_plural",
            "workflow_name": "release",
            "status": "completed",
            "ended_unix_ms": 10_u64
        }),
    );

    chat.dispatch_command(SlashCommand::Workflow);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one /workflow info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(rendered.contains("Workflows:"), "{rendered}");
    assert!(rendered.contains("Runtime:"), "{rendered}");
    assert!(rendered.contains("Approval: auto"), "{rendered}");
    assert!(!rendered.contains("Recent runs:"), "{rendered}");
    assert!(
        !rendered.contains("wf_visible_only_in_plural"),
        "{rendered}"
    );
}

#[tokio::test]
async fn slash_quit_requests_exit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Quit);

    assert_matches!(rx.try_recv(), Ok(AppEvent::Exit(ExitMode::ShutdownFirst)));
}

#[tokio::test]
async fn slash_logout_requests_app_server_logout() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Logout);

    assert_matches!(rx.try_recv(), Ok(AppEvent::Logout));
}

#[tokio::test]
async fn slash_copy_state_tracks_turn_complete_final_reply() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    complete_turn_with_message(&mut chat, "turn-1", Some("Final reply **markdown**"));

    assert_eq!(
        chat.last_agent_markdown_text(),
        Some("Final reply **markdown**")
    );
}

#[tokio::test]
async fn slash_copy_state_tracks_plan_item_completion() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let plan_text = "## Plan\n\n1. Build it\n2. Test it".to_string();

    chat.handle_server_notification(
        ServerNotification::ItemCompleted(ItemCompletedNotification {
            thread_id: String::new(),
            turn_id: "turn-1".to_string(),
            completed_at_ms: 0,
            item: AppServerThreadItem::Plan {
                id: "plan-1".to_string(),
                text: plan_text.clone(),
            },
        }),
        /*replay_kind*/ None,
    );
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);

    assert_eq!(chat.last_agent_markdown_text(), Some(plan_text.as_str()));
    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response == &plan_text
    );
}

#[tokio::test]
async fn slash_copy_reports_when_no_agent_response_exists() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Copy);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert_chatwidget_snapshot!("slash_copy_no_output_info_message", rendered);
    assert!(
        rendered.contains("No agent response to copy"),
        "expected no-output message, got {rendered:?}"
    );
}

#[tokio::test]
async fn ctrl_o_copy_reports_when_no_agent_response_exists() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("No agent response to copy"),
        "expected no-output message, got {rendered:?}"
    );
}

#[tokio::test]
async fn keymap_capture_can_capture_current_copy_shortcut() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let runtime_keymap = crate::keymap::RuntimeKeymap::defaults();
    chat.open_keymap_capture(
        "composer".to_string(),
        "submit".to_string(),
        crate::app_event::KeymapEditIntent::ReplaceAll,
        &runtime_keymap,
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));

    let AppEvent::KeymapCaptured {
        context,
        action,
        key,
        intent,
    } = rx.try_recv().expect("captured key event")
    else {
        panic!("expected keymap capture event");
    };
    assert_eq!(context, "composer");
    assert_eq!(action, "submit");
    assert_eq!(key, "ctrl-o");
    assert_eq!(intent, crate::app_event::KeymapEditIntent::ReplaceAll);
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "copy shortcut should not run while key capture is active"
    );
}

#[tokio::test]
async fn slash_keymap_capture_can_capture_app_shortcuts() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let runtime_keymap = crate::keymap::RuntimeKeymap::defaults();

    for (key, expected) in [('t', "ctrl-t"), ('l', "ctrl-l"), ('g', "ctrl-g")] {
        chat.open_keymap_capture(
            "global".to_string(),
            "open_transcript".to_string(),
            crate::app_event::KeymapEditIntent::ReplaceAll,
            &runtime_keymap,
        );

        chat.handle_key_event(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL));

        let AppEvent::KeymapCaptured {
            context,
            action,
            key,
            intent,
        } = rx.try_recv().expect("captured key event")
        else {
            panic!("expected keymap capture event");
        };
        assert_eq!(context, "global");
        assert_eq!(action, "open_transcript");
        assert_eq!(key, expected);
        assert_eq!(intent, crate::app_event::KeymapEditIntent::ReplaceAll);
    }
}

#[tokio::test]
async fn slash_keymap_debug_opens_keypress_inspector() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Keymap, "debug".to_string(), Vec::new());

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert!(popup.contains("Keypress Inspector"));
    assert!(popup.contains("Waiting for a keypress"));
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    let popup = render_bottom_popup(&chat, /*width*/ 100);
    assert!(popup.contains("global.copy (Copy)"));
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "debug inspector should open without transcript messages"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_keymap_debug_can_inspect_app_shortcuts() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Keymap, "debug".to_string(), Vec::new());

    for (key, expected_action) in [
        ('t', "global.open_transcript (Open Transcript)"),
        ('l', "global.clear_terminal (Clear Terminal)"),
        ('g', "global.open_external_editor (Open External Editor)"),
    ] {
        chat.handle_key_event(KeyEvent::new(KeyCode::Char(key), KeyModifiers::CONTROL));

        let popup = render_bottom_popup(&chat, /*width*/ 100);
        assert!(
            popup.contains(expected_action),
            "expected {expected_action:?} in debug popup for ctrl-{key}, got {popup:?}"
        );
    }

    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "debug inspector should not run app shortcut side effects"
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_keymap_invalid_args_show_usage() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/keymap nope");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /keymap [debug]"),
        "expected usage message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/keymap nope");
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn copy_shortcut_can_be_remapped() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let mut keymap_config = chat.config_ref().tui_keymap.clone();
    keymap_config.global.copy = Some(codex_config::types::KeybindingsSpec::One(
        codex_config::types::KeybindingSpec("ctrl-x".to_string()),
    ));
    let runtime_keymap =
        crate::keymap::RuntimeKeymap::from_config(&keymap_config).expect("valid copy remap");
    chat.apply_keymap_update(keymap_config, &runtime_keymap);

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('o'), KeyModifiers::CONTROL));
    assert!(
        drain_insert_history(&mut rx).is_empty(),
        "old copy shortcut should no longer copy"
    );

    chat.handle_key_event(KeyEvent::new(KeyCode::Char('x'), KeyModifiers::CONTROL));
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one info message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("No agent response to copy"),
        "expected remapped copy shortcut to run, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_copy_stores_clipboard_lease_and_preserves_it_on_failure() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.transcript.last_agent_markdown = Some("copy me".to_string());

    chat.copy_last_agent_markdown_with(|markdown| {
        assert_eq!(markdown, "copy me");
        Ok(Some(crate::clipboard_copy::ClipboardLease::test()))
    });

    assert!(chat.clipboard_lease.is_some());
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one success message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Copied last message to clipboard"),
        "expected success message, got {rendered:?}"
    );

    chat.copy_last_agent_markdown_with(|markdown| {
        assert_eq!(markdown, "copy me");
        Err("blocked".into())
    });

    assert!(chat.clipboard_lease.is_some());
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected one failure message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Copy failed: blocked"),
        "expected failure message, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_copy_state_is_preserved_during_running_task() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    complete_turn_with_message(&mut chat, "turn-1", Some("Previous completed reply"));
    chat.on_task_started();

    assert_eq!(
        chat.last_agent_markdown_text(),
        Some("Previous completed reply")
    );
}

#[tokio::test]
async fn slash_copy_uses_agent_message_item_when_turn_complete_omits_final_text() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    handle_turn_started(&mut chat, "turn-1");
    complete_assistant_message(
        &mut chat,
        "msg-1",
        "Legacy item final message",
        /*phase*/ None,
    );
    let _ = drain_insert_history(&mut rx);
    handle_turn_completed(&mut chat, "turn-1", /*duration_ms*/ None);
    let _ = drain_insert_history(&mut rx);

    assert_eq!(
        chat.last_agent_markdown_text(),
        Some("Legacy item final message")
    );
    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response == "Legacy item final message"
    );
}

#[tokio::test]
async fn agent_turn_complete_notification_does_not_reuse_stale_copy_source() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    complete_turn_with_message(&mut chat, "turn-1", Some("Previous reply"));
    chat.pending_notification = None;

    handle_turn_completed(&mut chat, "turn-2", /*duration_ms*/ None);

    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response.is_empty()
    );
}

#[tokio::test]
async fn active_goal_without_follow_up_suppresses_agent_turn_complete_notification() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::Goals, /*enabled*/ true);
    chat.handle_server_notification(
        ServerNotification::ThreadGoalUpdated(
            codex_app_server_protocol::ThreadGoalUpdatedNotification {
                thread_id: "thread-1".to_string(),
                turn_id: None,
                goal: codex_app_server_protocol::ThreadGoal {
                    thread_id: "thread-1".to_string(),
                    objective: "finish the benchmark".to_string(),
                    status: codex_app_server_protocol::ThreadGoalStatus::Active,
                    token_budget: None,
                    tokens_used: 0,
                    time_used_seconds: 0,
                    created_at: 1,
                    updated_at: 1,
                },
            },
        ),
        /*replay_kind*/ None,
    );

    complete_turn_with_message(&mut chat, "turn-1", Some("Still working"));

    assert_matches!(chat.pending_notification, None);
}

#[tokio::test]
async fn queued_follow_up_suppresses_agent_turn_complete_notification() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");
    chat.queue_user_message("Continue".into());

    complete_turn_with_message(&mut chat, "turn-1", Some("Still working"));

    assert_matches!(chat.pending_notification, None);
    assert!(chat.input_queue.queued_user_messages.is_empty());
    assert_matches!(next_submit_op(&mut op_rx), Op::UserTurn { .. });
}

#[tokio::test]
async fn queued_menu_slash_keeps_agent_turn_complete_notification() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.2")).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");
    queue_composer_text_with_tab(&mut chat, "/model");

    complete_turn_with_message(&mut chat, "turn-1", Some("Done"));

    assert_matches!(
        chat.pending_notification,
        Some(Notification::AgentTurnComplete { ref response }) if response == "Done"
    );
    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Select Model"));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn slash_copy_uses_latest_surviving_response_after_rollback() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    replay_user_message_text(&mut chat, "user-1", "foo", ReplayKind::ThreadSnapshot);
    replay_agent_message(
        &mut chat,
        "agent-1",
        "foo response",
        ReplayKind::ThreadSnapshot,
    );
    replay_user_message_text(&mut chat, "user-2", "bar", ReplayKind::ThreadSnapshot);
    replay_agent_message(
        &mut chat,
        "agent-2",
        "bar response",
        ReplayKind::ThreadSnapshot,
    );
    let _ = drain_insert_history(&mut rx);
    assert_eq!(chat.last_agent_markdown_text(), Some("bar response"));

    chat.truncate_agent_copy_history_to_user_turn_count(/*user_turn_count*/ 1);

    assert_eq!(chat.last_agent_markdown_text(), Some("foo response"));
    chat.copy_last_agent_markdown_with(|markdown| {
        assert_eq!(markdown, "foo response");
        Ok(None)
    });
}

#[tokio::test]
async fn slash_copy_reports_when_rewind_exceeds_retained_copy_history() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    replay_user_message_text(&mut chat, "user-1", "foo", ReplayKind::ThreadSnapshot);
    replay_agent_message(
        &mut chat,
        "agent-1",
        "foo response",
        ReplayKind::ThreadSnapshot,
    );
    let _ = drain_insert_history(&mut rx);

    chat.truncate_agent_copy_history_to_user_turn_count(/*user_turn_count*/ 0);
    chat.dispatch_command(SlashCommand::Copy);

    let cells = drain_insert_history(&mut rx);
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(
            "Cannot copy that response after rewinding. Only the most recent 32 responses are available to /copy."
        ),
        "expected evicted-history message, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_exit_requests_exit() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Exit);

    assert_matches!(rx.try_recv(), Ok(AppEvent::Exit(ExitMode::ShutdownFirst)));
}

#[tokio::test]
async fn slash_stop_submits_background_terminal_cleanup() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Stop);

    assert_matches!(op_rx.try_recv(), Ok(Op::CleanBackgroundTerminals));
    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected cleanup confirmation message");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("Stopping all background terminals."),
        "expected cleanup confirmation, got {rendered:?}"
    );
}

#[tokio::test]
async fn slash_clear_requests_ui_clear_when_idle() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Clear);

    assert_matches!(rx.try_recv(), Ok(AppEvent::ClearUi));
}

#[tokio::test]
async fn slash_clear_after_ctrl_c_keeps_stashed_draft_recallable() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);
    chat.bottom_pane
        .set_history_metadata(thread_id, /*log_id*/ 1, /*entry_count*/ 0);

    submit_composer_text(&mut chat, "ok");
    assert_eq!(next_add_to_history_event(&mut rx), "ok");

    let stashed_draft = "explain why history recall lost this draft";

    chat.bottom_pane
        .set_composer_text(stashed_draft.to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Char('c'), KeyModifiers::CONTROL));
    assert_eq!(chat.bottom_pane.composer_text(), "");
    assert_eq!(next_add_to_history_event(&mut rx), stashed_draft);

    chat.bottom_pane
        .set_composer_text("/clear".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));

    assert_matches!(rx.try_recv(), Ok(AppEvent::ClearUi));
    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), stashed_draft);

    chat.handle_key_event(KeyEvent::new(KeyCode::Up, KeyModifiers::NONE));
    assert_eq!(chat.bottom_pane.composer_text(), "ok");
}

#[tokio::test]
async fn slash_clear_is_disabled_while_task_running() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.dispatch_command(SlashCommand::Clear);

    let event = rx.try_recv().expect("expected disabled command error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains("'/clear' is disabled while a task is in progress."),
                "expected /clear task-running error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected no follow-up events");
}

#[tokio::test]
async fn slash_archive_is_disabled_while_task_running() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.bottom_pane.set_task_running(/*running*/ true);

    chat.dispatch_command(SlashCommand::Archive);

    let event = rx.try_recv().expect("expected disabled command error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(
                rendered.contains("'/archive' is disabled while a task is in progress."),
                "expected /archive task-running error, got {rendered:?}"
            );
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(rx.try_recv().is_err(), "expected no follow-up events");
}

#[tokio::test]
async fn slash_memory_drop_reports_stubbed_feature() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::MemoryDrop);

    let event = rx.try_recv().expect("expected unsupported-feature error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(rendered.contains("Memory maintenance: Not available in TUI yet."));
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(
        op_rx.try_recv().is_err(),
        "expected no memory op to be sent"
    );
}

#[tokio::test]
async fn slash_mcp_requests_inventory_via_app_server() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    chat.dispatch_command(SlashCommand::Mcp);

    assert!(active_blob(&chat).contains("Loading MCP inventory"));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::FetchMcpInventory {
            detail: McpServerStatusDetail::ToolsAndAuthOnly,
            thread_id: Some(actual_thread_id)
        }) if actual_thread_id == thread_id
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_verbose_requests_full_inventory_via_app_server() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    submit_composer_text(&mut chat, "/mcp verbose");

    assert!(active_blob(&chat).contains("Loading MCP inventory"));
    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::FetchMcpInventory {
            detail: McpServerStatusDetail::Full,
            thread_id: Some(actual_thread_id)
        }) if actual_thread_id == thread_id
    );
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_mcp_invalid_args_show_usage() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    submit_composer_text(&mut chat, "/mcp full");

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|cell| lines_to_single_string(cell))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /mcp [verbose]"),
        "expected usage message, got: {rendered:?}"
    );
    assert_eq!(recall_latest_after_clearing(&mut chat), "/mcp full");
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_memories_opens_memory_menu() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.set_feature_enabled(Feature::MemoryTool, /*enabled*/ true);

    chat.dispatch_command(SlashCommand::Memories);

    assert!(render_bottom_popup(&chat, /*width*/ 80).contains("Use memories"));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert!(op_rx.try_recv().is_err(), "expected no core op to be sent");
}

#[tokio::test]
async fn slash_memory_update_reports_stubbed_feature() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::MemoryUpdate);

    let event = rx.try_recv().expect("expected unsupported-feature error");
    match event {
        AppEvent::InsertHistoryCell(cell) => {
            let rendered = lines_to_single_string(&cell.display_lines(/*width*/ 80));
            assert!(rendered.contains("Memory maintenance: Not available in TUI yet."));
        }
        other => panic!("expected InsertHistoryCell error, got {other:?}"),
    }
    assert!(
        op_rx.try_recv().is_err(),
        "expected no memory op to be sent"
    );
}

#[tokio::test]
async fn slash_resume_opens_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Resume);

    assert_matches!(rx.try_recv(), Ok(AppEvent::OpenResumePicker));
}

#[tokio::test]
async fn slash_import_opens_claude_code_import_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Import);

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenExternalAgentConfigMigration)
    );
}

#[tokio::test]
async fn slash_archive_confirmation_requests_current_thread_archive() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Archive);

    assert!(chat.bottom_pane.has_active_view());
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_archive_confirmation_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::ArchiveCurrentThread));
}

#[tokio::test]
async fn slash_delete_confirmation_requests_current_thread_delete() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Delete);

    assert!(chat.bottom_pane.has_active_view());
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_delete_confirmation_popup", popup);

    chat.handle_key_event(KeyEvent::from(KeyCode::Down));
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::DeleteCurrentThread));
}

#[tokio::test]
async fn slash_resume_with_arg_requests_named_session() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.bottom_pane.set_composer_text(
        "/resume my-saved-thread".to_string(),
        Vec::new(),
        Vec::new(),
    );
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::ResumeSessionByIdOrName(id_or_name)) if id_or_name == "my-saved-thread"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_opens_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_pet_image_support(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(chat.bottom_pane.has_active_view());
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));

    let popup = render_bottom_popup(&chat, /*width*/ 80);
    assert_chatwidget_snapshot!("slash_pets_picker", popup);
}

#[tokio::test]
#[serial]
async fn slash_pets_with_arg_selects_named_pet() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_pet_image_support(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pets chefito".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::PetSelected { pet_id }) if pet_id == "chefito"
    );
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_disable_disables_pets_even_on_unsupported_terminal() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pets disable".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::PetDisabled));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pet_hide_disables_pets_even_on_unsupported_terminal() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pet hide".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    assert_matches!(rx.try_recv(), Ok(AppEvent::PetDisabled));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_on_unsupported_terminal_warns_without_picker() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(!chat.bottom_pane.has_active_view());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets are disabled in tmux."));
    assert!(rendered.contains("outside tmux"));
}

#[tokio::test]
#[serial]
async fn slash_pets_with_arg_on_unsupported_terminal_warns_without_selection() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_tmux_pet_image_unsupported(&mut chat);

    chat.bottom_pane
        .set_composer_text("/pets chefito".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets are disabled in tmux."));
    assert_matches!(rx.try_recv(), Err(TryRecvError::Empty));
    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
#[serial]
async fn slash_pets_on_unsupported_terminal_shows_terminal_warning() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_terminal_pet_image_unsupported(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(!chat.bottom_pane.has_active_view());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets aren’t available in this terminal."));
    assert!(rendered.contains("Kitty graphics or Sixel support"));
}

#[tokio::test]
#[serial]
async fn slash_pets_on_old_iterm2_shows_upgrade_warning() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    force_old_iterm2_pet_image_unsupported(&mut chat);

    chat.dispatch_command(SlashCommand::Pets);

    assert!(!chat.bottom_pane.has_active_view());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(rendered.contains("Pets require iTerm2 3.6 or newer."));
    assert!(rendered.contains("Upgrade iTerm2 to use terminal pets."));
}

#[tokio::test]
async fn slash_fork_requests_current_fork() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Fork);

    assert_matches!(rx.try_recv(), Ok(AppEvent::ForkCurrentSession));
}

#[tokio::test]
async fn slash_app_requests_desktop_handoff() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let thread_id = ThreadId::new();
    chat.thread_id = Some(thread_id);

    chat.dispatch_command(SlashCommand::App);

    assert_matches!(
        rx.try_recv(),
        Ok(AppEvent::OpenDesktopThread {
            thread_id: actual_thread_id,
        }) if actual_thread_id == thread_id
    );
}

#[tokio::test]
async fn slash_app_without_thread_id_shows_starting_error() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::App);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected app startup error");
    assert_chatwidget_snapshot!(
        "slash_app_without_thread_id_shows_starting_error",
        lines_to_single_string(&cells[0])
    );
}

#[tokio::test]
async fn slash_rollout_displays_current_path() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    let rollout_path = PathBuf::from("/tmp/codex-test-rollout.jsonl");
    chat.current_rollout_path = Some(rollout_path.clone());

    chat.dispatch_command(SlashCommand::Rollout);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(cells.len(), 1, "expected info message for rollout path");
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains(&rollout_path.display().to_string()),
        "expected rollout path to be shown: {rendered}"
    );
}

#[tokio::test]
async fn slash_rollout_handles_missing_path() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Rollout);

    let cells = drain_insert_history(&mut rx);
    assert_eq!(
        cells.len(),
        1,
        "expected info message explaining missing path"
    );
    let rendered = lines_to_single_string(&cells[0]);
    assert!(
        rendered.contains("not available"),
        "expected missing rollout path message: {rendered}"
    );
}

#[tokio::test]
async fn fast_slash_command_updates_and_persists_local_service_tier() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode override app event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier),
            }
            if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode persistence app event; events: {events:?}"
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn fast_keybinding_toggle_uses_same_events_as_fast_slash_command() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.toggle_fast_mode_from_ui();

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode override app event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier),
            }
            if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected fast-mode persistence app event; events: {events:?}"
    );

    assert_matches!(op_rx.try_recv(), Err(TryRecvError::Empty));
}

#[tokio::test]
async fn fast_keybinding_toggle_requires_feature_and_idle_surface() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ false);

    assert!(!chat.can_toggle_fast_mode_from_keybinding());

    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);
    assert!(chat.can_toggle_fast_mode_from_keybinding());

    chat.bottom_pane.set_task_running(/*running*/ true);
    assert!(!chat.can_toggle_fast_mode_from_keybinding());
}

#[tokio::test]
async fn user_turn_carries_service_tier_after_fast_toggle() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());

    let _events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

    chat.bottom_pane
        .set_composer_text("hello".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == ServiceTier::Fast.request_value() => {}
        other => panic!("expected Op::UserTurn with fast service tier, got {other:?}"),
    }
}

#[tokio::test]
async fn disabled_ultracode_keyword_trigger_does_not_set_workflow_mode() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    chat.config.workflows.keyword_trigger_enabled = false;

    submit_composer_text(&mut chat, "please use ultracode for this");

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            collaboration_mode,
            items,
            ..
        } => {
            let collaboration_mode = collaboration_mode.expect("default collaboration mode");
            assert_eq!(WorkflowMode::Disabled, collaboration_mode.workflow_mode());
            assert_eq!(None, collaboration_mode.reasoning_effort());
            assert_eq!(
                items,
                vec![UserInput::Text {
                    text: "please use ultracode for this".to_string(),
                    text_elements: Vec::new(),
                }]
            );
        }
        other => panic!("expected normal user turn, got {other:?}"),
    }
}

#[tokio::test]
async fn model_switch_recomputes_catalog_default_service_tier() {
    let (mut chat, _rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.3-codex")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    let mut models = chat.model_catalog.try_list_models().expect("test catalog");
    let default_model = models
        .iter_mut()
        .find(|model| model.model == "gpt-5.4")
        .expect("gpt-5.4 test model");
    default_model.default_service_tier = Some(ServiceTier::Fast.request_value().to_string());
    chat.model_catalog = std::sync::Arc::new(ModelCatalog::new(models));
    chat.refresh_effective_service_tier();

    assert_eq!(chat.current_service_tier(), None);

    chat.set_model("gpt-5.4");
    assert_eq!(
        chat.current_service_tier(),
        Some(ServiceTier::Fast.request_value())
    );

    chat.set_model("gpt-5.3-codex");
    assert_eq!(chat.current_service_tier(), None);

    chat.bottom_pane
        .set_composer_text("hello".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE => {}
        other => panic!("expected Op::UserTurn with default service tier override, got {other:?}"),
    }
}

#[tokio::test]
async fn queued_fast_slash_applies_before_next_queued_message() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);
    handle_turn_started(&mut chat, "turn-1");

    queue_composer_text_with_tab(&mut chat, "/fast");
    queue_composer_text_with_tab(&mut chat, "hello after fast");

    complete_turn_with_message(&mut chat, "turn-1", Some("done"));

    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == ServiceTier::Fast.request_value()
        )),
        "expected queued /fast to update service tier before next turn; events: {events:?}"
    );

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            items,
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == ServiceTier::Fast.request_value() => assert_eq!(
            items,
            vec![UserInput::Text {
                text: "hello after fast".to_string(),
                text_elements: Vec::new(),
            }]
        ),
        other => panic!("expected queued message to submit with fast tier, got {other:?}"),
    }
}

#[tokio::test]
async fn user_turn_sends_standard_override_after_fast_is_turned_off() {
    let (mut chat, mut rx, mut op_rx) = make_chatwidget_manual(Some("gpt-5.4")).await;
    chat.thread_id = Some(ThreadId::new());
    set_chatgpt_auth(&mut chat);
    set_fast_mode_test_catalog(&mut chat);
    chat.set_feature_enabled(Feature::FastMode, /*enabled*/ true);

    chat.handle_service_tier_command_dispatch(fast_tier_command());
    let _events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();

    chat.handle_service_tier_command_dispatch(fast_tier_command());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::CodexOp(Op::OverrideTurnContext {
                service_tier: Some(Some(service_tier)),
                ..
            }) if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE
        )),
        "expected fast-mode off default service tier app event; events: {events:?}"
    );
    assert!(
        events.iter().any(|event| matches!(
            event,
            AppEvent::PersistServiceTierSelection {
                service_tier: Some(service_tier)
            } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE
        )),
        "expected default service tier persistence app event; events: {events:?}"
    );

    chat.bottom_pane
        .set_composer_text("hello".to_string(), Vec::new(), Vec::new());
    chat.handle_key_event(KeyEvent::from(KeyCode::Enter));

    match next_submit_op(&mut op_rx) {
        Op::UserTurn {
            service_tier: Some(Some(service_tier)),
            ..
        } if service_tier == SERVICE_TIER_DEFAULT_REQUEST_VALUE => {}
        other => panic!("expected Op::UserTurn with default service tier override, got {other:?}"),
    }
}

#[tokio::test]
async fn raw_slash_command_toggles_and_accepts_on_off_args() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command(SlashCommand::Raw);
    assert!(chat.raw_output_mode());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::RawOutputModeChanged { enabled: true }))
    );

    chat.dispatch_command_with_args(SlashCommand::Raw, "off".to_string(), Vec::new());
    assert!(!chat.raw_output_mode());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::RawOutputModeChanged { enabled: false }))
    );

    chat.dispatch_command_with_args(SlashCommand::Raw, "on".to_string(), Vec::new());
    assert!(chat.raw_output_mode());
    let events = std::iter::from_fn(|| rx.try_recv().ok()).collect::<Vec<_>>();
    assert!(
        events
            .iter()
            .any(|event| matches!(event, AppEvent::RawOutputModeChanged { enabled: true }))
    );
}

#[tokio::test]
async fn raw_slash_command_reports_usage_for_invalid_arg() {
    let (mut chat, mut rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;

    chat.dispatch_command_with_args(SlashCommand::Raw, "status".to_string(), Vec::new());

    assert!(!chat.raw_output_mode());
    let cells = drain_insert_history(&mut rx);
    let rendered = cells
        .iter()
        .map(|lines| lines_to_single_string(lines))
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        rendered.contains("Usage: /raw [on|off]"),
        "expected raw usage error, got {rendered:?}"
    );
}

#[tokio::test]
async fn compact_queues_user_messages_snapshot() {
    let (mut chat, _rx, _op_rx) = make_chatwidget_manual(/*model_override*/ None).await;
    chat.thread_id = Some(ThreadId::new());
    handle_turn_started(&mut chat, "turn-1");

    chat.submit_user_message(UserMessage::from(
        "Steer submitted while /compact was running.".to_string(),
    ));
    handle_error(
        &mut chat,
        "cannot steer a compact turn",
        Some(CodexErrorInfo::ActiveTurnNotSteerable {
            turn_kind: NonSteerableTurnKind::Compact,
        }),
    );

    let width: u16 = 80;
    let height: u16 = 18;
    let backend = VT100Backend::new(width, height);
    let mut term = crate::custom_terminal::Terminal::with_options(backend).expect("terminal");
    let desired_height = chat.desired_height(width).min(height);
    term.set_viewport_area(Rect::new(0, height - desired_height, width, desired_height));
    term.draw(|f| {
        chat.render(f.area(), f.buffer_mut());
    })
    .unwrap();
    assert_chatwidget_snapshot!(
        "compact_queues_user_messages_snapshot",
        normalize_snapshot_paths(term.backend().vt100().screen().contents())
    );
}
