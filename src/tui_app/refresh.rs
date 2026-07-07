use super::*;
use crate::cli::{execute_snapshot_refresh, fetch_diff, fetch_diff_for_path, fetch_file_summary};
use crate::tui_data::{load_changes, load_operations};
use crate::tui_support::{clamp_index, is_root_commit, non_empty_or};

impl TuiApp {
    pub fn refresh_all(&mut self) {
        self.observed_at = self.observed_at.saturating_add(1);
        match load_operations() {
            Ok((version, current_head, operations)) => {
                self.version = version;
                self.current_head = current_head;
                self.operations = operations;
                self.selected_operation =
                    clamp_index(self.selected_operation, self.operations.len());
                self.refresh_changes();
                self.set_browse_status(format!(
                    "gen {} sync; stale async results: none",
                    self.observed_at
                ));
            }
            Err(error) => self.status = error,
        }
    }

    pub fn snapshot_then_refresh(&mut self) {
        match execute_snapshot_refresh() {
            Ok(result) => {
                self.push_command_log(
                    "jj util snapshot".to_string(),
                    Some(result.exit_code),
                    "snapshot ok".to_string(),
                );
                self.refresh_all();
                self.set_browse_status("snapshot refreshed");
            }
            Err(error) => {
                self.push_command_log("jj util snapshot".to_string(), None, "failed".to_string());
                self.status = format!("snapshot failed: {error}");
            }
        }
    }

    pub fn refresh_after_mutation(&mut self, preferred_change_id: Option<&str>) {
        self.refresh_all();
        if let Some(index) = self
            .operations
            .iter()
            .position(|operation| operation.is_current)
        {
            self.selected_operation = index;
            self.refresh_changes();
        }
        if let Some(change_id) = preferred_change_id
            && let Some(index) = self
                .changes
                .iter()
                .position(|change| change.change_id == change_id)
        {
            self.selected_change = index;
            self.refresh_files();
        }
    }

    pub fn refresh_changes(&mut self) {
        self.changes.clear();
        self.files.clear();
        let Some(op_id) = self.selected_operation_id() else {
            return;
        };
        match load_changes(&self.version, &op_id) {
            Ok(changes) => {
                self.changes = changes;
                self.selected_change = 0;
                self.refresh_files();
            }
            Err(error) => self.status = error,
        }
    }

    pub fn refresh_files(&mut self) {
        self.files.clear();
        self.selected_file = 0;
        self.selected_files.clear();
        self.file_selection_anchor = None;
        let Some(op_id) = self.selected_operation_id() else {
            return;
        };
        let Some(commit_id) = self.selected_commit_id() else {
            return;
        };
        if is_root_commit(&commit_id) {
            self.files = vec![TuiFile {
                path: String::new(),
                label: "ROOT has no file diff".to_string(),
            }];
            self.select_file(0, false, false);
            self.detail = vec!["ROOT [immutable]".to_string()];
            self.detail_title = "ROOT".to_string();
            return;
        }
        match fetch_file_summary(&op_id, &commit_id) {
            Ok(files) => {
                self.files = if files.is_empty() {
                    vec![TuiFile {
                        path: String::new(),
                        label: "(no files)".to_string(),
                    }]
                } else {
                    files
                        .into_iter()
                        .map(|file| TuiFile {
                            label: format!("{} {}", file.status, file.path),
                            path: file.path,
                        })
                        .collect()
                };
                self.selected_file = 0;
                self.file_scroll = 0;
                self.select_file(0, false, false);
                self.refresh_diff();
            }
            Err(error) => self.status = format!("failed to fetch files: {error}"),
        }
    }

    pub fn refresh_diff(&mut self) {
        let Some(op_id) = self.selected_operation_id() else {
            return;
        };
        let Some(commit_id) = self.selected_commit_id() else {
            return;
        };
        if is_root_commit(&commit_id) {
            return;
        }
        let path = self
            .files
            .get(self.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("");
        let diff = if path.is_empty() {
            fetch_diff(&op_id, &commit_id)
        } else {
            fetch_diff_for_path(&op_id, &commit_id, path)
        };
        match diff {
            Ok(diff) => {
                self.detail_title = if path.is_empty() {
                    format!("4 Diff {}", short(&commit_id))
                } else {
                    format!("4 Diff {} {}", short(&commit_id), path)
                };
                self.detail =
                    non_empty_or(diff.lines().map(str::to_string).collect(), "(empty diff)");
                self.detail_scroll = 0;
            }
            Err(error) => self.status = format!("failed to fetch diff: {error}"),
        }
    }
}
