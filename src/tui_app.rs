mod modal;
mod mutations;
mod navigation;
mod refresh;
mod render;

use std::collections::BTreeSet;

use crate::pending_mutation::PendingMutation;
use crate::state::{CommandKind, Freshness, ObservationContext, ViewMode, guard_command};
use crate::tui::UiPane;
use crate::tui_data::{SelectedChange, TuiChange, TuiFile, TuiOperation};
use crate::tui_support::short;

pub struct TuiApp {
    pub mode: UiMode,
    pub pane: UiPane,
    pub operations: Vec<TuiOperation>,
    pub changes: Vec<TuiChange>,
    pub files: Vec<TuiFile>,
    pub detail_title: String,
    pub detail: Vec<String>,
    pub modal: ActiveModal,
    pub maximized_pane: Option<UiPane>,
    pub operation_pane_collapsed: bool,
    pub selected_operation: usize,
    pub selected_change: usize,
    pub selected_file: usize,
    pub selected_files: BTreeSet<usize>,
    pub file_selection_anchor: Option<usize>,
    pub operation_scroll: usize,
    pub operation_visible_rows: usize,
    pub change_scroll: usize,
    pub change_visible_rows: usize,
    pub file_scroll: usize,
    pub file_visible_rows: usize,
    pub detail_scroll: u16,
    pub version: String,
    pub current_head: String,
    pub observed_at: u64,
    pub status: String,
    pub command_log: Vec<CommandLogEntry>,
    pub pinned_change: Option<SelectedChange>,
}

pub struct OverlayPane {
    pub title: String,
    pub lines: Vec<String>,
    pub scroll: u16,
}

pub struct InputCursor {
    pub line: u16,
    pub column: u16,
}

pub struct CommandLogEntry {
    pub command: String,
    pub exit_code: Option<i32>,
    pub status: String,
}

pub enum ActiveModal {
    None,
    Overlay(OverlayPane),
    Describe {
        input: DescribeInput,
        scroll: u16,
    },
    Commit {
        input: CommitInput,
        scroll: u16,
    },
    Bookmark {
        input: BookmarkInput,
        scroll: u16,
    },
    Workspace {
        input: WorkspaceInput,
        scroll: u16,
    },
    PendingMutation {
        mutation: PendingMutation,
        scroll: u16,
    },
    Rebase {
        source: SelectedChange,
        scroll: u16,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    Browse,
    Surgery,
}

pub struct DescribeInput {
    pub change_id: String,
    pub commit_id: String,
    pub message: String,
}

pub struct CommitInput {
    pub message: String,
}

pub struct BookmarkInput {
    pub change: SelectedChange,
    pub name: String,
}

pub struct WorkspaceInput {
    pub path: String,
}

impl Default for TuiApp {
    fn default() -> Self {
        Self::new()
    }
}

impl TuiApp {
    pub fn new() -> Self {
        let mut app = Self {
            mode: UiMode::Browse,
            pane: UiPane::Operation,
            operations: Vec::new(),
            changes: Vec::new(),
            files: Vec::new(),
            detail_title: "4 Detail".to_string(),
            detail: vec![
                "Enter: show focused item here".to_string(),
                "u: undo preview  Ctrl-r: redo preview  R: snapshot preview".to_string(),
            ],
            modal: ActiveModal::None,
            maximized_pane: None,
            operation_pane_collapsed: false,
            selected_operation: 0,
            selected_change: 0,
            selected_file: 0,
            selected_files: BTreeSet::new(),
            file_selection_anchor: None,
            operation_scroll: 0,
            operation_visible_rows: 1,
            change_scroll: 0,
            change_visible_rows: 1,
            file_scroll: 0,
            file_visible_rows: 1,
            detail_scroll: 0,
            version: String::new(),
            current_head: "?".to_string(),
            observed_at: 0,
            status: "browse mode; mutations require y confirmation".to_string(),
            command_log: Vec::new(),
            pinned_change: None,
        };
        app.refresh_all();
        app
    }

    pub fn selected_operation_id(&self) -> Option<String> {
        self.operations
            .get(self.selected_operation)
            .map(|operation| operation.id.clone())
    }

    pub fn current_operation(&self) -> Option<&TuiOperation> {
        self.operations
            .iter()
            .find(|operation| operation.is_current)
    }

    pub fn is_viewing_current_operation(&self) -> bool {
        self.operations
            .get(self.selected_operation)
            .map(|operation| operation.is_current)
            .unwrap_or(false)
    }

    pub fn view_mode(&self) -> &'static str {
        if self.is_viewing_current_operation() {
            "current"
        } else {
            "historical"
        }
    }

    pub fn historical_warning(&self) -> Option<String> {
        if self.is_viewing_current_operation() {
            return None;
        }
        Some(format!(
            "HISTORICAL VIEW - mutations disabled | current op {} | viewing op {}",
            self.current_operation()
                .map(|operation| short(&operation.id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.selected_operation_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string())
        ))
    }

    pub fn observation_context(&self) -> ObservationContext {
        let selected = self
            .selected_operation_id()
            .unwrap_or_else(|| "?".to_string());
        let current = self
            .current_operation()
            .map(|operation| operation.id.clone())
            .unwrap_or_else(|| self.current_head.clone());
        ObservationContext {
            view_mode: if self.is_viewing_current_operation() {
                ViewMode::FollowCurrent
            } else {
                ViewMode::Historical(selected.clone())
            },
            observed_operation: selected,
            current_operation: current,
            freshness: Freshness::Current,
        }
    }

    pub fn operation_context_lines(&self) -> Vec<String> {
        let current = self
            .current_operation()
            .map(|operation| short(&operation.id).to_string())
            .unwrap_or_else(|| "?".to_string());
        let viewing = self
            .selected_operation_id()
            .map(|id| short(&id).to_string())
            .unwrap_or_else(|| "?".to_string());
        vec![
            format!("current op: {current}"),
            format!("viewing op: {viewing} ({})", self.view_mode()),
        ]
    }

    pub fn block_historical_mutation(&mut self, action: &str) -> bool {
        self.block_command(action, CommandKind::MutateCurrent)
    }

    pub fn block_command(&mut self, action: &str, kind: CommandKind) -> bool {
        let Err(error) = guard_command(&self.observation_context(), kind) else {
            return false;
        };
        let mut lines = self.operation_context_lines();
        lines.push(String::new());
        lines.push(format!("{action} is blocked: {}.", error.reason));
        lines.push(
            "Normal mutations require the current operation; historical views can only import into current."
                .to_string(),
        );
        lines.push(
            "OP pane Enter only shows operation details; it does not change mutation target."
                .to_string(),
        );
        self.show_overlay("Historical View", lines);
        self.status = self
            .historical_warning()
            .unwrap_or_else(|| "historical view: mutation blocked".to_string());
        true
    }

    pub fn set_browse_status(&mut self, status: impl Into<String>) {
        self.status = self.historical_warning().unwrap_or_else(|| status.into());
    }

    pub fn selected_commit_id(&self) -> Option<String> {
        self.changes
            .get(self.selected_change)
            .map(|change| change.commit_id.clone())
    }

    pub fn selected_change(&self) -> Option<SelectedChange> {
        self.changes
            .get(self.selected_change)
            .map(SelectedChange::from)
    }

    pub fn workspace_change(&self) -> Option<SelectedChange> {
        self.changes
            .iter()
            .find(|change| change.is_workspace)
            .map(SelectedChange::from)
    }

    pub fn change_by_commit(&self, commit_id: &str) -> Option<SelectedChange> {
        self.changes
            .iter()
            .find(|change| change.commit_id == commit_id)
            .map(SelectedChange::from)
    }

    pub fn descendants_of(&self, commit_id: &str) -> Vec<String> {
        let mut pending = vec![commit_id.to_string()];
        let mut descendants = Vec::new();
        while let Some(parent) = pending.pop() {
            for change in self
                .changes
                .iter()
                .filter(|change| change.parent_commit_ids.iter().any(|id| id == &parent))
            {
                if descendants
                    .iter()
                    .any(|id: &String| id == &change.commit_id)
                {
                    continue;
                }
                descendants.push(change.brief());
                pending.push(change.commit_id.clone());
            }
        }
        descendants
    }

    pub fn selected_file_path(&self) -> &str {
        self.files
            .get(self.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("")
    }

    pub fn toggle_maximize_pane(&mut self) {
        self.maximized_pane = match self.maximized_pane {
            Some(pane) if pane == self.pane => None,
            _ => Some(self.pane),
        };
        self.set_browse_status(match self.maximized_pane {
            Some(_) => "pane maximized",
            None => "pane restored",
        });
    }

    pub fn collapse_operation_pane(&mut self) {
        self.operation_pane_collapsed = true;
        if self.pane == UiPane::Operation || self.pane == UiPane::Repository {
            self.pane = UiPane::Change;
        }
        self.set_browse_status("operation pane collapsed");
    }

    pub fn restore_operation_pane(&mut self) {
        self.operation_pane_collapsed = false;
        self.set_browse_status("operation pane restored");
    }

    pub fn enter_surgery_mode(&mut self) {
        self.mode = UiMode::Surgery;
        self.operation_pane_collapsed = false;
        self.pane = UiPane::Operation;
        self.status = "operation graph surgery; Operation pane actions only".to_string();
    }

    pub fn exit_surgery_mode(&mut self) {
        self.mode = UiMode::Browse;
        self.set_browse_status("left surgery mode");
    }

    pub fn selected_editor_file(&self) -> Result<String, String> {
        let path = self.selected_file_path();
        if path.is_empty() {
            return Err("no file selected for editor".to_string());
        }
        let absolute = std::env::current_dir()
            .map_err(|error| format!("failed to resolve repo directory: {error}"))?
            .join(path);
        if !absolute.exists() {
            return Err(format!(
                "selected file is not present in working tree: {}",
                absolute.display()
            ));
        }
        absolute
            .canonicalize()
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| format!("failed to resolve file path: {error}"))
    }
}
