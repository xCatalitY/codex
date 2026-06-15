use crossterm::event::KeyCode;
use crossterm::event::KeyEvent;
use crossterm::event::KeyModifiers;
use ratatui::buffer::Buffer;
use ratatui::layout::Constraint;
use ratatui::layout::Layout;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::Line;
use ratatui::widgets::Block;
use ratatui::widgets::Widget;

use codex_config::types::WorkflowApproval;
use codex_protocol::config_types::WorkflowMode;

use crate::app_event::AppEvent;
use crate::app_event_sender::AppEventSender;
use crate::bottom_pane::popup_consts::MAX_POPUP_ROWS;
use crate::key_hint;
use crate::key_hint::KeyBindingListExt;
use crate::keymap::ListKeymap;
use crate::render::Insets;
use crate::render::RectExt as _;
use crate::render::renderable::ColumnRenderable;
use crate::render::renderable::Renderable;
use crate::style::user_message_style;

use super::CancellationEvent;
use super::bottom_pane_view::BottomPaneView;
use super::scroll_state::ScrollState;
use super::selection_popup_common::GenericDisplayRow;
use super::selection_popup_common::measure_rows_height;
use super::selection_popup_common::render_rows;
use super::workflow_settings_model::WorkflowNamedPolicyItem;
use super::workflow_settings_model::next_optional_workflow_approval;
use super::workflow_settings_model::next_workflow_approval;
use super::workflow_settings_model::next_workflow_enabled_override;
use super::workflow_settings_model::next_workflow_mode;
use super::workflow_settings_model::workflow_approval_label;
use super::workflow_settings_model::workflow_enabled_override_label;
use super::workflow_settings_model::workflow_mode_label;
use super::workflow_settings_model::workflow_optional_approval_label;

const WORKFLOW_SETTINGS_VIEW_ID: &str = "workflow-settings";

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkflowSetting {
    Runtime,
    Mode,
    Approval,
    KeywordTrigger,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum WorkflowSettingRow {
    Setting(WorkflowSetting),
    NamedEnabled(usize),
    NamedApproval(usize),
    AddNamedPolicy,
}

pub(crate) struct WorkflowSettingsView {
    enabled: bool,
    mode: WorkflowMode,
    approval: WorkflowApproval,
    keyword_trigger_enabled: bool,
    named_policies: Vec<WorkflowNamedPolicyItem>,
    original_named_policies: Vec<WorkflowNamedPolicyItem>,
    new_policy_name_input: Option<String>,
    notice: Option<String>,
    state: ScrollState,
    complete: bool,
    app_event_tx: AppEventSender,
    keymap: ListKeymap,
}

impl WorkflowSettingsView {
    pub(crate) fn new(
        enabled: bool,
        mode: WorkflowMode,
        approval: WorkflowApproval,
        keyword_trigger_enabled: bool,
        named_policies: Vec<WorkflowNamedPolicyItem>,
        app_event_tx: AppEventSender,
        keymap: ListKeymap,
    ) -> Self {
        let mut state = ScrollState::new();
        state.selected_idx = Some(0);
        let original_named_policies = named_policies.clone();
        Self {
            enabled,
            mode,
            approval,
            keyword_trigger_enabled,
            named_policies,
            original_named_policies,
            new_policy_name_input: None,
            notice: None,
            state,
            complete: false,
            app_event_tx,
            keymap,
        }
    }

    fn settings() -> [WorkflowSetting; 4] {
        [
            WorkflowSetting::Runtime,
            WorkflowSetting::Mode,
            WorkflowSetting::Approval,
            WorkflowSetting::KeywordTrigger,
        ]
    }

    fn row_kinds(&self) -> Vec<WorkflowSettingRow> {
        let mut rows = Self::settings()
            .into_iter()
            .map(WorkflowSettingRow::Setting)
            .collect::<Vec<_>>();
        for index in 0..self.named_policies.len() {
            rows.push(WorkflowSettingRow::NamedEnabled(index));
            rows.push(WorkflowSettingRow::NamedApproval(index));
        }
        rows.push(WorkflowSettingRow::AddNamedPolicy);
        rows
    }

    fn settings_header(&self) -> ColumnRenderable<'_> {
        let mut header = ColumnRenderable::new();
        header.push(Line::from("Workflows".bold()));
        header.push(Line::from(
            "Configure workflow defaults. Changes are saved to config.toml".dim(),
        ));
        if !self.named_policies.is_empty() {
            header.push(Line::from(
                "Named workflow overrides apply to exact names, including plugin:name entries."
                    .dim(),
            ));
        }
        if let Some(input) = self.new_policy_name_input.as_ref() {
            header.push(Line::from(format!("New named override: {input}_").bold()));
        }
        if let Some(notice) = self.notice.as_ref() {
            header.push(Line::from(notice.clone().dim()));
        }
        header
    }

    fn visible_len(&self) -> usize {
        self.row_kinds().len()
    }

    fn build_rows(&self) -> Vec<GenericDisplayRow> {
        let selected_idx = self.state.selected_idx;
        self.row_kinds()
            .into_iter()
            .enumerate()
            .map(|(idx, row)| {
                let prefix = if selected_idx == Some(idx) {
                    '›'
                } else {
                    ' '
                };
                let (name, description) = match row {
                    WorkflowSettingRow::Setting(WorkflowSetting::Runtime) => (
                        format!(
                            "{prefix} [{}] Workflow runtime",
                            if self.enabled { 'x' } else { ' ' }
                        ),
                        "Expose the workflow tool when workflow mode is active.",
                    ),
                    WorkflowSettingRow::Setting(WorkflowSetting::Mode) => (
                        format!(
                            "{prefix} Default mode: {}",
                            workflow_mode_label(self.mode)
                        ),
                        "Applied to new sessions and to the current session after saving.",
                    ),
                    WorkflowSettingRow::Setting(WorkflowSetting::Approval) => (
                        format!(
                            "{prefix} Approval: {}",
                            workflow_approval_label(self.approval)
                        ),
                        "Controls whether workflow scripts run automatically, ask first, or are denied.",
                    ),
                    WorkflowSettingRow::Setting(WorkflowSetting::KeywordTrigger) => (
                        format!(
                            "{prefix} [{}] Ultracode keyword trigger",
                            if self.keyword_trigger_enabled { 'x' } else { ' ' }
                        ),
                        "Let the prompt keyword ultracode enable one-turn xhigh workflow orchestration.",
                    ),
                    WorkflowSettingRow::NamedEnabled(index) => {
                        let item = &self.named_policies[index];
                        (
                            format!(
                                "{prefix} {} enabled: {}",
                                item.name,
                                workflow_enabled_override_label(item.enabled)
                            ),
                            "Cycles inherited, enabled, and disabled. Disabled blocks the named workflow before script execution.",
                        )
                    }
                    WorkflowSettingRow::NamedApproval(index) => {
                        let item = &self.named_policies[index];
                        (
                            format!(
                                "{prefix} {} approval: {}",
                                item.name,
                                workflow_optional_approval_label(item.approval)
                            ),
                            "Cycles inherited, auto, ask, allow, and deny for this exact workflow name.",
                        )
                    }
                    WorkflowSettingRow::AddNamedPolicy => {
                        if let Some(input) = self.new_policy_name_input.as_ref() {
                            (
                                format!("{prefix} Add named workflow override: {input}_"),
                                "Type an exact workflow name, then press Enter to add policy rows.",
                            )
                        } else {
                            (
                                format!("{prefix} Add named workflow override..."),
                                "Add policy rows for a workflow that is not currently discovered.",
                            )
                        }
                    }
                };
                GenericDisplayRow {
                    name,
                    description: Some(description.to_string()),
                    ..Default::default()
                }
            })
            .collect()
    }

    fn move_up(&mut self) {
        let len = self.visible_len();
        self.state.move_up_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn move_down(&mut self) {
        let len = self.visible_len();
        self.state.move_down_wrap(len);
        self.state.ensure_visible(len, MAX_POPUP_ROWS.min(len));
    }

    fn page_up(&mut self) {
        let len = self.visible_len();
        self.state.page_up_clamped(len, MAX_POPUP_ROWS.min(len));
    }

    fn page_down(&mut self) {
        let len = self.visible_len();
        self.state.page_down_clamped(len, MAX_POPUP_ROWS.min(len));
    }

    fn jump_top(&mut self) {
        let len = self.visible_len();
        self.state.jump_top(len, MAX_POPUP_ROWS.min(len));
    }

    fn jump_bottom(&mut self) {
        let len = self.visible_len();
        self.state.jump_bottom(len, MAX_POPUP_ROWS.min(len));
    }

    fn activate_selected(&mut self) {
        let Some(selected_idx) = self.state.selected_idx else {
            return;
        };
        let Some(row) = self.row_kinds().get(selected_idx).copied() else {
            return;
        };
        match row {
            WorkflowSettingRow::Setting(WorkflowSetting::Runtime) => self.enabled = !self.enabled,
            WorkflowSettingRow::Setting(WorkflowSetting::Mode) => {
                self.mode = next_workflow_mode(self.mode)
            }
            WorkflowSettingRow::Setting(WorkflowSetting::Approval) => {
                self.approval = next_workflow_approval(self.approval)
            }
            WorkflowSettingRow::Setting(WorkflowSetting::KeywordTrigger) => {
                self.keyword_trigger_enabled = !self.keyword_trigger_enabled
            }
            WorkflowSettingRow::NamedEnabled(index) => {
                self.named_policies[index].enabled =
                    next_workflow_enabled_override(self.named_policies[index].enabled);
            }
            WorkflowSettingRow::NamedApproval(index) => {
                self.named_policies[index].approval =
                    next_optional_workflow_approval(self.named_policies[index].approval);
            }
            WorkflowSettingRow::AddNamedPolicy => self.start_new_policy_input(),
        }
    }

    fn add_named_policy_row_index(&self) -> usize {
        self.visible_len().saturating_sub(1)
    }

    fn start_new_policy_input(&mut self) {
        self.new_policy_name_input = Some(String::new());
        self.notice = Some("Type an exact workflow name; whitespace is not allowed.".to_string());
        self.state.selected_idx = Some(self.add_named_policy_row_index());
    }

    fn cancel_new_policy_input(&mut self) {
        self.new_policy_name_input = None;
        self.notice = None;
    }

    fn commit_new_policy_input(&mut self) {
        let Some(input) = self.new_policy_name_input.take() else {
            return;
        };
        let name = input.trim().to_string();
        if name.is_empty() {
            self.notice = Some("Workflow name is required.".to_string());
            self.new_policy_name_input = Some(String::new());
            self.state.selected_idx = Some(self.add_named_policy_row_index());
            return;
        }
        if name.chars().any(|ch| ch.is_control() || ch.is_whitespace()) {
            self.notice =
                Some("Workflow names in this form cannot contain whitespace.".to_string());
            self.new_policy_name_input = Some(name);
            self.state.selected_idx = Some(self.add_named_policy_row_index());
            return;
        }
        if let Some(existing_index) = self
            .named_policies
            .iter()
            .position(|item| item.name == name)
        {
            self.notice = Some(format!("Workflow `{name}` already has policy rows."));
            self.state.selected_idx = Some(Self::settings().len() + existing_index * 2);
            return;
        }

        self.named_policies.push(WorkflowNamedPolicyItem {
            name: name.clone(),
            enabled: Some(true),
            approval: None,
        });
        self.named_policies
            .sort_by(|left, right| left.name.cmp(&right.name));
        let inserted_index = self
            .named_policies
            .iter()
            .position(|item| item.name == name)
            .unwrap_or_else(|| self.named_policies.len().saturating_sub(1));
        self.notice = Some(format!(
            "Added `{name}` with an explicit enabled override; cycle approval if needed."
        ));
        self.state.selected_idx = Some(Self::settings().len() + inserted_index * 2);
    }

    fn handle_new_policy_input_key_event(&mut self, key_event: KeyEvent) {
        match key_event {
            _ if self.keymap.accept.is_pressed(key_event) => self.commit_new_policy_input(),
            _ if self.keymap.cancel.is_pressed(key_event) => self.cancel_new_policy_input(),
            KeyEvent {
                code: KeyCode::Backspace,
                ..
            } => {
                if let Some(input) = self.new_policy_name_input.as_mut() {
                    input.pop();
                }
            }
            KeyEvent {
                code: KeyCode::Char(ch),
                modifiers,
                ..
            } if modifiers.is_empty() || modifiers == KeyModifiers::SHIFT => {
                if let Some(input) = self.new_policy_name_input.as_mut() {
                    input.push(ch);
                }
            }
            _ => {}
        }
    }

    fn save(&mut self) {
        self.app_event_tx.send(AppEvent::UpdateWorkflowSettings {
            enabled: self.enabled,
            mode: self.mode,
            approval: self.approval,
            keyword_trigger_enabled: self.keyword_trigger_enabled,
        });
        for item in &self.named_policies {
            let original = self
                .original_named_policies
                .iter()
                .find(|original| original.name == item.name);
            let original_enabled = original.and_then(|item| item.enabled);
            if original_enabled != item.enabled {
                self.app_event_tx
                    .send(AppEvent::UpdateNamedWorkflowEnabled {
                        workflow_name: item.name.clone(),
                        enabled: item.enabled,
                    });
            }
            let original_approval = original.and_then(|item| item.approval);
            if original_approval != item.approval {
                self.app_event_tx
                    .send(AppEvent::UpdateNamedWorkflowApproval {
                        workflow_name: item.name.clone(),
                        approval: item.approval,
                    });
            }
        }
        self.complete = true;
    }

    fn cancel(&mut self) {
        self.complete = true;
    }

    fn rows_width(total_width: u16) -> u16 {
        total_width.saturating_sub(2)
    }
}

impl BottomPaneView for WorkflowSettingsView {
    fn handle_key_event(&mut self, key_event: KeyEvent) {
        if self.new_policy_name_input.is_some() {
            self.handle_new_policy_input_key_event(key_event);
            return;
        }
        match key_event {
            _ if self.keymap.move_up.is_pressed(key_event) => self.move_up(),
            _ if self.keymap.move_down.is_pressed(key_event) => self.move_down(),
            _ if self.keymap.page_up.is_pressed(key_event) => self.page_up(),
            _ if self.keymap.page_down.is_pressed(key_event) => self.page_down(),
            _ if self.keymap.jump_top.is_pressed(key_event) => self.jump_top(),
            _ if self.keymap.jump_bottom.is_pressed(key_event) => self.jump_bottom(),
            KeyEvent {
                code: KeyCode::Char('a'),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.start_new_policy_input(),
            KeyEvent {
                code: KeyCode::Char(' '),
                modifiers: KeyModifiers::NONE,
                ..
            } => self.activate_selected(),
            _ if self.keymap.accept.is_pressed(key_event) => self.save(),
            _ if self.keymap.cancel.is_pressed(key_event) => self.cancel(),
            _ => {}
        }
    }

    fn is_complete(&self) -> bool {
        self.complete
    }

    fn on_ctrl_c(&mut self) -> CancellationEvent {
        self.cancel();
        CancellationEvent::Handled
    }

    fn view_id(&self) -> Option<&'static str> {
        Some(WORKFLOW_SETTINGS_VIEW_ID)
    }
}

impl Renderable for WorkflowSettingsView {
    fn render(&self, area: Rect, buf: &mut Buffer) {
        if area.height == 0 || area.width == 0 {
            return;
        }

        let [content_area, footer_area] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);

        Block::default()
            .style(user_message_style())
            .render(content_area, buf);

        let header = self.settings_header();
        let header_height = header.desired_height(content_area.width.saturating_sub(4));
        let rows = self.build_rows();
        let rows_width = Self::rows_width(content_area.width);
        let rows_height = measure_rows_height(
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );
        let [header_area, _, list_area] = Layout::vertical([
            Constraint::Max(header_height),
            Constraint::Max(1),
            Constraint::Length(rows_height),
        ])
        .areas(content_area.inset(Insets::vh(/*v*/ 1, /*h*/ 2)));

        header.render(header_area, buf);
        if list_area.height > 0 {
            let render_area = Rect {
                x: list_area.x.saturating_sub(2),
                y: list_area.y,
                width: rows_width.max(1),
                height: list_area.height,
            };
            render_rows(
                render_area,
                buf,
                &rows,
                &self.state,
                MAX_POPUP_ROWS,
                "  No workflow settings available",
            );
        }

        let hint_area = Rect {
            x: footer_area.x + 2,
            y: footer_area.y,
            width: footer_area.width.saturating_sub(2),
            height: footer_area.height,
        };
        workflow_settings_hint_line(self.new_policy_name_input.is_some()).render(hint_area, buf);
    }

    fn desired_height(&self, width: u16) -> u16 {
        let header = self.settings_header();
        let rows = self.build_rows();
        let rows_width = Self::rows_width(width);
        let rows_height = measure_rows_height(
            &rows,
            &self.state,
            MAX_POPUP_ROWS,
            rows_width.saturating_add(1),
        );

        let mut height = header.desired_height(width.saturating_sub(4));
        height = height.saturating_add(rows_height + 4);
        height.saturating_add(1)
    }
}

fn workflow_settings_hint_line(input_active: bool) -> Line<'static> {
    if input_active {
        return Line::from(vec![
            "Type workflow name; ".into(),
            key_hint::plain(KeyCode::Enter).into(),
            " to add; ".into(),
            key_hint::plain(KeyCode::Esc).into(),
            " to cancel".into(),
        ]);
    }
    Line::from(vec![
        "Press ".into(),
        key_hint::plain(KeyCode::Char(' ')).into(),
        " to toggle/cycle; ".into(),
        key_hint::plain(KeyCode::Char('a')).into(),
        " to add named override; ".into(),
        key_hint::plain(KeyCode::Enter).into(),
        " to save".into(),
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::keymap::RuntimeKeymap;
    use tokio::sync::mpsc::unbounded_channel;

    fn test_sender() -> (
        AppEventSender,
        tokio::sync::mpsc::UnboundedReceiver<AppEvent>,
    ) {
        let (tx, rx) = unbounded_channel();
        (AppEventSender::new(tx), rx)
    }

    fn named_item(
        name: &str,
        enabled: Option<bool>,
        approval: Option<WorkflowApproval>,
    ) -> WorkflowNamedPolicyItem {
        WorkflowNamedPolicyItem {
            name: name.to_string(),
            enabled,
            approval,
        }
    }

    #[test]
    fn build_rows_includes_named_workflow_policy_controls() {
        let (sender, _rx) = test_sender();
        let view = WorkflowSettingsView::new(
            true,
            WorkflowMode::Ultracode,
            WorkflowApproval::Auto,
            true,
            vec![named_item(
                "sample:release",
                Some(false),
                Some(WorkflowApproval::Ask),
            )],
            sender,
            RuntimeKeymap::defaults().list,
        );

        let rows = view.build_rows();
        let row_names = rows.iter().map(|row| row.name.as_str()).collect::<Vec<_>>();

        assert!(
            row_names
                .iter()
                .any(|name| name.contains("sample:release enabled: disabled")),
            "{row_names:?}"
        );
        assert!(
            row_names
                .iter()
                .any(|name| name.contains("sample:release approval: ask")),
            "{row_names:?}"
        );
        assert!(
            row_names
                .iter()
                .any(|name| name.contains("Add named workflow override")),
            "{row_names:?}"
        );
    }

    #[test]
    fn save_emits_only_changed_named_workflow_policy_updates() {
        let (sender, mut rx) = test_sender();
        let mut view = WorkflowSettingsView::new(
            true,
            WorkflowMode::Ultracode,
            WorkflowApproval::Auto,
            true,
            vec![named_item(
                "sample:release",
                Some(false),
                Some(WorkflowApproval::Ask),
            )],
            sender,
            RuntimeKeymap::defaults().list,
        );

        view.state.selected_idx = Some(4);
        view.handle_key_event(KeyEvent::from(KeyCode::Char(' ')));
        view.state.selected_idx = Some(5);
        view.handle_key_event(KeyEvent::from(KeyCode::Char(' ')));
        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        match rx.try_recv().expect("workflow settings update") {
            AppEvent::UpdateWorkflowSettings {
                enabled,
                mode,
                approval,
                keyword_trigger_enabled,
            } => {
                assert!(enabled);
                assert_eq!(mode, WorkflowMode::Ultracode);
                assert_eq!(approval, WorkflowApproval::Auto);
                assert!(keyword_trigger_enabled);
            }
            event => panic!("expected workflow settings update, got {event:?}"),
        }
        match rx.try_recv().expect("named enabled update") {
            AppEvent::UpdateNamedWorkflowEnabled {
                workflow_name,
                enabled,
            } => {
                assert_eq!(workflow_name, "sample:release");
                assert_eq!(enabled, None);
            }
            event => panic!("expected named enabled update, got {event:?}"),
        }
        match rx.try_recv().expect("named approval update") {
            AppEvent::UpdateNamedWorkflowApproval {
                workflow_name,
                approval,
            } => {
                assert_eq!(workflow_name, "sample:release");
                assert_eq!(approval, Some(WorkflowApproval::Allow));
            }
            event => panic!("expected named approval update, got {event:?}"),
        }
        assert!(rx.try_recv().is_err());
        assert!(view.is_complete());
    }

    #[test]
    fn add_named_workflow_policy_from_input_emits_enabled_update_on_save() {
        let (sender, mut rx) = test_sender();
        let mut view = WorkflowSettingsView::new(
            true,
            WorkflowMode::Ultracode,
            WorkflowApproval::Auto,
            true,
            Vec::new(),
            sender,
            RuntimeKeymap::defaults().list,
        );

        view.state.selected_idx = Some(4);
        view.handle_key_event(KeyEvent::from(KeyCode::Char(' ')));
        for ch in "adhoc:release".chars() {
            view.handle_key_event(KeyEvent::from(KeyCode::Char(ch)));
        }
        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        assert_eq!(
            view.named_policies,
            vec![named_item("adhoc:release", Some(true), None)]
        );
        assert_eq!(view.state.selected_idx, Some(4));

        view.handle_key_event(KeyEvent::from(KeyCode::Enter));
        match rx.try_recv().expect("workflow settings update") {
            AppEvent::UpdateWorkflowSettings { .. } => {}
            event => panic!("expected workflow settings update, got {event:?}"),
        }
        match rx.try_recv().expect("named enabled update") {
            AppEvent::UpdateNamedWorkflowEnabled {
                workflow_name,
                enabled,
            } => {
                assert_eq!(workflow_name, "adhoc:release");
                assert_eq!(enabled, Some(true));
            }
            event => panic!("expected named enabled update, got {event:?}"),
        }
        assert!(rx.try_recv().is_err());
        assert!(view.is_complete());
    }

    #[test]
    fn duplicate_named_workflow_policy_input_selects_existing_rows() {
        let (sender, _rx) = test_sender();
        let mut view = WorkflowSettingsView::new(
            true,
            WorkflowMode::Ultracode,
            WorkflowApproval::Auto,
            true,
            vec![named_item("release", None, Some(WorkflowApproval::Ask))],
            sender,
            RuntimeKeymap::defaults().list,
        );

        view.handle_key_event(KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE));
        for ch in "release".chars() {
            view.handle_key_event(KeyEvent::from(KeyCode::Char(ch)));
        }
        view.handle_key_event(KeyEvent::from(KeyCode::Enter));

        assert_eq!(view.named_policies.len(), 1);
        assert_eq!(view.state.selected_idx, Some(4));
        assert!(
            view.notice
                .as_deref()
                .is_some_and(|notice| notice.contains("already has policy rows"))
        );
    }
}
