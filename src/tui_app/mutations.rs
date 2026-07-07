use super::*;
use crate::mutation::{abandon_preview, execute_mutation};
use crate::tui::{preview_panel_for_abandon, render_preview_panel};
use crate::tui_support::{command_result_lines, is_root_commit};

impl TuiApp {
    pub fn preview_surgery_abandon_selected_operation(&mut self) {
        let Some(target) = self.selected_operation_id() else {
            self.status = "no operation selected".to_string();
            return;
        };
        let current = self
            .current_operation()
            .map(|operation| operation.id.as_str());
        let preview = abandon_preview(vec![target.clone()], current);
        let mut lines =
            render_preview_panel(&preview_panel_for_abandon(&preview), &preview.warnings)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>();
        lines.insert(0, "direct execution blocked".to_string());
        lines.insert(0, "operation graph surgery preview".to_string());
        lines.insert(0, format!("target op: {}", short(&target)));
        self.show_overlay("Surgery preview abandon", lines);
        self.status = "surgery preview only; direct execution blocked".to_string();
    }

    pub fn plan_undo(&mut self) {
        if self.block_historical_mutation("undo") {
            return;
        }
        self.set_pending_mutation(PendingMutation::Undo);
    }

    pub fn plan_edit_selected(&mut self) {
        if self.block_historical_mutation("edit") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot edit ROOT".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Edit { change });
    }

    pub fn plan_new_child_selected(&mut self) {
        if self.block_historical_mutation("new child") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot create child from ROOT".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::NewChild { change });
    }

    pub fn plan_squash_current(&mut self) {
        if self.block_historical_mutation("squash") {
            return;
        }
        let Some(source) = self.workspace_change().or_else(|| self.selected_change()) else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&source.commit_id) {
            self.status = "cannot squash ROOT".to_string();
            return;
        }
        let destination = self
            .changes
            .iter()
            .find(|change| {
                change.commit_id == source.commit_id
                    && change.parent_commit_ids.len() == 1
                    && !is_root_commit(&change.parent_commit_ids[0])
            })
            .and_then(|change| self.change_by_commit(&change.parent_commit_ids[0]))
            .or_else(|| {
                self.changes
                    .iter()
                    .find(|change| change.commit_id == source.commit_id)
                    .and_then(|change| change.parent_commit_ids.first())
                    .and_then(|id| self.change_by_commit(id))
            });
        let Some(destination) = destination else {
            self.status = "squash destination parent not found".to_string();
            return;
        };
        self.set_pending_mutation(PendingMutation::SquashCurrent {
            source,
            destination,
        });
    }

    pub fn plan_abandon_selected(&mut self) {
        if self.block_historical_mutation("abandon") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot abandon ROOT".to_string();
            return;
        }
        let descendants = self.descendants_of(&change.commit_id);
        let bookmarks = self
            .changes
            .get(self.selected_change)
            .map(|change| change.bookmarks.clone())
            .unwrap_or_default();
        self.set_pending_mutation(PendingMutation::Abandon {
            change,
            descendants,
            bookmarks,
        });
    }

    pub fn begin_rebase_selected(&mut self) {
        if self.block_historical_mutation("rebase") {
            return;
        }
        let Some(source) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&source.commit_id) {
            self.status = "cannot rebase ROOT".to_string();
            return;
        }
        self.modal = ActiveModal::Rebase { source, scroll: 0 };
        self.pane = UiPane::Change;
        self.status = "select rebase destination".to_string();
    }

    pub fn finish_rebase_selection(&mut self) {
        if self.block_historical_mutation("rebase") {
            self.modal = ActiveModal::None;
            return;
        }
        let ActiveModal::Rebase { source, .. } =
            std::mem::replace(&mut self.modal, ActiveModal::None)
        else {
            return;
        };
        let Some(destination) = self.selected_change() else {
            self.status = "no rebase destination selected".to_string();
            return;
        };
        if source.commit_id == destination.commit_id {
            self.status = "source and destination are the same change".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Rebase {
            source,
            destination,
        });
    }

    pub fn plan_restore_selected(&mut self) {
        if self.block_historical_mutation("restore") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot restore ROOT".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Restore { change });
    }

    pub fn plan_import_historical_file(&mut self) {
        if self.block_command("import historical file", CommandKind::ImportFromHistorical) {
            return;
        }
        let Some(source_op) = self.selected_operation_id() else {
            self.status = "no historical operation selected".to_string();
            return;
        };
        let Some(source) = self.selected_change() else {
            self.status = "no source change selected".to_string();
            return;
        };
        if is_root_commit(&source.commit_id) {
            self.status = "cannot import from ROOT".to_string();
            return;
        }
        let mut paths = self
            .selected_files
            .iter()
            .filter_map(|index| self.files.get(*index))
            .map(|file| file.path.clone())
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            let path = self.selected_file_path().to_string();
            if !path.is_empty() {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            self.status = "no file selected for import".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::ImportHistoricalFile {
            source_op,
            source,
            paths,
        });
    }

    pub fn begin_describe_selected(&mut self) {
        if self.block_historical_mutation("describe") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot describe ROOT".to_string();
            return;
        }
        self.modal = ActiveModal::Describe {
            input: DescribeInput {
                change_id: change.change_id.clone(),
                commit_id: change.commit_id.clone(),
                message: String::new(),
            },
            scroll: 0,
        };
        self.status = "describe input".to_string();
    }

    pub fn push_describe_char(&mut self, ch: char) {
        if let ActiveModal::Describe { input, .. } = &mut self.modal {
            input.message.push(ch);
        }
    }

    pub fn pop_describe_char(&mut self) {
        if let ActiveModal::Describe { input, .. } = &mut self.modal {
            input.message.pop();
        }
    }

    pub fn cancel_describe_input(&mut self) {
        self.modal = ActiveModal::None;
        self.refresh_diff();
        self.status = "describe cancelled".to_string();
    }

    pub fn execute_describe_input(&mut self) {
        if self.block_historical_mutation("describe") {
            return;
        }
        let ActiveModal::Describe { input, .. } =
            std::mem::replace(&mut self.modal, ActiveModal::None)
        else {
            return;
        };
        if input.message.trim().is_empty() {
            self.status = "description is empty; cancelled".to_string();
            return;
        }
        let args = vec![
            "describe".to_string(),
            input.commit_id.clone(),
            "-m".to_string(),
            input.message,
        ];
        match execute_mutation(args.clone()) {
            Ok(result) => {
                let success = result.exit_code == 0;
                if success {
                    self.refresh_after_mutation(Some(&input.change_id));
                }
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    Some(result.exit_code),
                    if success { "ok" } else { "failed" }.to_string(),
                );
                self.show_overlay("Mutation describe", command_result_lines(&result));
                self.status = if success {
                    format!("executed jj {}", args.join(" "))
                } else {
                    format!("describe failed: exit {}", result.exit_code)
                };
            }
            Err(error) => {
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    None,
                    "spawn failed".to_string(),
                );
                self.show_overlay(
                    "Mutation describe",
                    vec![format!("failed to execute jj {}: {error}", args.join(" "))],
                );
                self.status = "describe execution failed".to_string();
            }
        }
    }

    pub fn begin_commit_input(&mut self) {
        if self.block_historical_mutation("commit") {
            return;
        }
        self.modal = ActiveModal::Commit {
            input: CommitInput {
                message: String::new(),
            },
            scroll: 0,
        };
        self.status = "commit input".to_string();
    }

    pub fn push_commit_char(&mut self, ch: char) {
        if let ActiveModal::Commit { input, .. } = &mut self.modal {
            input.message.push(ch);
        }
    }

    pub fn pop_commit_char(&mut self) {
        if let ActiveModal::Commit { input, .. } = &mut self.modal {
            input.message.pop();
        }
    }

    pub fn cancel_commit_input(&mut self) {
        self.modal = ActiveModal::None;
        self.refresh_diff();
        self.status = "commit cancelled".to_string();
    }

    pub fn confirm_commit_input(&mut self) {
        if self.block_historical_mutation("commit") {
            return;
        }
        let ActiveModal::Commit { input, .. } =
            std::mem::replace(&mut self.modal, ActiveModal::None)
        else {
            return;
        };
        let message = input.message.trim().to_string();
        if message.is_empty() {
            self.status = "commit message is empty; cancelled".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Commit { message });
    }

    pub fn begin_bookmark_input(&mut self) {
        if self.block_historical_mutation("bookmark") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot bookmark ROOT".to_string();
            return;
        }
        self.modal = ActiveModal::Bookmark {
            input: BookmarkInput {
                change,
                name: String::new(),
            },
            scroll: 0,
        };
        self.status = "bookmark input".to_string();
    }

    pub fn push_bookmark_char(&mut self, ch: char) {
        if let ActiveModal::Bookmark { input, .. } = &mut self.modal {
            input.name.push(ch);
        }
    }

    pub fn pop_bookmark_char(&mut self) {
        if let ActiveModal::Bookmark { input, .. } = &mut self.modal {
            input.name.pop();
        }
    }

    pub fn cancel_bookmark_input(&mut self) {
        self.modal = ActiveModal::None;
        self.refresh_diff();
        self.status = "bookmark cancelled".to_string();
    }

    pub fn confirm_bookmark_input(&mut self) {
        if self.block_historical_mutation("bookmark") {
            return;
        }
        let ActiveModal::Bookmark { input, .. } =
            std::mem::replace(&mut self.modal, ActiveModal::None)
        else {
            return;
        };
        let name = input.name.trim().to_string();
        if name.is_empty() {
            self.status = "bookmark name is empty; cancelled".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::BookmarkSet {
            name,
            change: input.change,
        });
    }

    pub fn begin_workspace_input(&mut self) {
        if self.block_historical_mutation("workspace add") {
            return;
        }
        self.modal = ActiveModal::Workspace {
            input: WorkspaceInput {
                path: String::new(),
            },
            scroll: 0,
        };
        self.status = "workspace input".to_string();
    }

    pub fn push_workspace_char(&mut self, ch: char) {
        if let ActiveModal::Workspace { input, .. } = &mut self.modal {
            input.path.push(ch);
        }
    }

    pub fn pop_workspace_char(&mut self) {
        if let ActiveModal::Workspace { input, .. } = &mut self.modal {
            input.path.pop();
        }
    }

    pub fn cancel_workspace_input(&mut self) {
        self.modal = ActiveModal::None;
        self.refresh_diff();
        self.status = "workspace add cancelled".to_string();
    }

    pub fn confirm_workspace_input(&mut self) {
        if self.block_historical_mutation("workspace add") {
            return;
        }
        let ActiveModal::Workspace { input, .. } =
            std::mem::replace(&mut self.modal, ActiveModal::None)
        else {
            return;
        };
        let path = input.path.trim().to_string();
        if path.is_empty() {
            self.status = "workspace path is empty; cancelled".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::WorkspaceAdd { path });
    }

    pub fn plan_push(&mut self) {
        if self.block_historical_mutation("push") {
            return;
        }
        let bookmark = self
            .changes
            .get(self.selected_change)
            .and_then(TuiChange::local_bookmark);
        self.set_pending_mutation(PendingMutation::Push { bookmark });
    }

    pub fn set_pending_mutation(&mut self, mutation: PendingMutation) {
        if self.block_command(mutation.name(), mutation.command_kind()) {
            return;
        }
        self.status = "confirm mutation with y".to_string();
        self.modal = ActiveModal::PendingMutation {
            mutation,
            scroll: 0,
        };
    }

    pub fn cancel_pending_mutation(&mut self) {
        self.modal = ActiveModal::None;
        self.refresh_diff();
        self.status = "mutation cancelled".to_string();
    }

    pub fn execute_pending_mutation(&mut self) {
        let kind = match &self.modal {
            ActiveModal::PendingMutation { mutation, .. } => mutation.command_kind(),
            _ => {
                self.status = "no pending mutation".to_string();
                return;
            }
        };
        if self.block_command("execute mutation", kind) {
            return;
        }
        let ActiveModal::PendingMutation { mutation, .. } =
            std::mem::replace(&mut self.modal, ActiveModal::None)
        else {
            self.status = "no pending mutation".to_string();
            return;
        };
        let preferred_change_id = mutation.preferred_change_id().map(str::to_string);
        let args = mutation.command_args();
        let result = execute_mutation(args.clone());
        match result {
            Ok(result) => {
                let success = result.exit_code == 0;
                if success {
                    self.refresh_after_mutation(preferred_change_id.as_deref());
                }
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    Some(result.exit_code),
                    if success { "ok" } else { "failed" }.to_string(),
                );
                self.show_overlay(
                    format!("Mutation {}", mutation.name()),
                    command_result_lines(&result),
                );
                self.status = if success {
                    format!("executed jj {}", args.join(" "))
                } else {
                    format!("mutation failed: exit {}", result.exit_code)
                };
            }
            Err(error) => {
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    None,
                    "spawn failed".to_string(),
                );
                self.show_overlay(
                    format!("Mutation {}", mutation.name()),
                    vec![format!("failed to execute jj {}: {error}", args.join(" "))],
                );
                self.status = "mutation execution failed".to_string();
            }
        }
    }
}
