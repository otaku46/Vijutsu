use crossterm::event::KeyModifiers;

use super::*;
use crate::tui_support::{
    clamp_index, is_command_modifier, keep_selected_visible, move_index, pane_at,
};

impl TuiApp {
    pub fn focus_previous(&mut self) {
        self.pane = match self.pane {
            UiPane::Operation | UiPane::Repository => UiPane::Diff,
            UiPane::Change => UiPane::Operation,
            UiPane::Files => UiPane::Change,
            UiPane::Diff => UiPane::Files,
        };
        self.set_browse_status(format!("pane: {:?}", self.pane));
    }

    pub fn focus_at(
        &mut self,
        column: u16,
        row: u16,
        width: u16,
        height: u16,
        modifiers: KeyModifiers,
    ) {
        if self.has_modal() {
            return;
        }
        let body_height = height.saturating_sub(6);
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        let item_row = row.saturating_sub(1) as usize;
        self.pane = match pane_at(
            column,
            row,
            width,
            height,
            self.maximized_pane,
            self.operation_pane_collapsed,
        ) {
            Some(UiPane::Operation) => {
                self.selected_operation =
                    clamp_index(self.operation_scroll + item_row, self.operations.len());
                UiPane::Operation
            }
            Some(UiPane::Change) => {
                self.selected_change =
                    clamp_index(self.change_scroll + item_row, self.changes.len());
                UiPane::Change
            }
            Some(UiPane::Files) => {
                let row_in_files = if matches!(self.maximized_pane, Some(UiPane::Files)) {
                    item_row
                } else {
                    let files_top = body_height.saturating_mul(55) / 100;
                    row.saturating_sub(files_top).saturating_sub(1) as usize
                };
                let target = clamp_index(self.file_scroll + row_in_files, self.files.len());
                self.select_file(target, is_command_modifier(modifiers), false);
                UiPane::Files
            }
            Some(UiPane::Diff) => UiPane::Diff,
            _ => self.pane,
        };
        self.sync_after_selection(old_operation, old_change, old_file);
        self.set_browse_status(format!("pane: {:?}", self.pane));
    }

    pub fn scroll_at(&mut self, column: u16, row: u16, width: u16, height: u16, delta: isize) {
        if self.has_modal() {
            self.scroll_overlay_page((delta * 3) as i16);
            return;
        }
        self.pane = pane_at(
            column,
            row,
            width,
            height,
            self.maximized_pane,
            self.operation_pane_collapsed,
        )
        .unwrap_or(self.pane);
        match self.pane {
            UiPane::Diff => self.scroll_detail_page((delta * 3) as i16),
            _ => self.move_selection(delta),
        }
    }

    pub fn sync_after_selection(
        &mut self,
        old_operation: usize,
        old_change: usize,
        old_file: usize,
    ) {
        if self.selected_operation != old_operation {
            self.refresh_changes();
            self.keep_selected_operation_visible();
        } else if self.selected_change != old_change {
            self.refresh_files();
            self.keep_selected_change_visible();
        } else if self.selected_file != old_file {
            self.refresh_diff();
        } else {
            return;
        }
        self.detail_scroll = 0;
        self.set_browse_status("selection updated");
    }

    pub fn move_selection(&mut self, delta: isize) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                self.selected_operation =
                    move_index(self.selected_operation, self.operations.len(), delta);
                self.keep_selected_operation_visible();
            }
            UiPane::Change => {
                self.selected_change = move_index(self.selected_change, self.changes.len(), delta);
                self.keep_selected_change_visible();
            }
            UiPane::Files => {
                self.selected_file = move_index(self.selected_file, self.files.len(), delta);
                self.select_file(self.selected_file, false, false);
                self.keep_selected_file_visible();
            }
            UiPane::Diff => {
                if delta < 0 {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                } else {
                    self.detail_scroll = self.detail_scroll.saturating_add(1);
                }
            }
        }
        self.sync_after_selection(old_operation, old_change, old_file);
    }

    pub fn jump_selection(&mut self, end: bool) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                self.selected_operation = if end {
                    self.operations.len().saturating_sub(1)
                } else {
                    0
                };
                self.keep_selected_operation_visible();
            }
            UiPane::Change => {
                self.selected_change = if end {
                    self.changes.len().saturating_sub(1)
                } else {
                    0
                };
                self.keep_selected_change_visible();
            }
            UiPane::Files => {
                let target = if end {
                    self.files.len().saturating_sub(1)
                } else {
                    0
                };
                self.select_file(target, false, false);
                self.keep_selected_file_visible();
            }
            UiPane::Diff => {}
        }
        self.sync_after_selection(old_operation, old_change, old_file);
    }

    pub fn move_graph_relative(&mut self, to_parent: bool) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                if let Some(index) = self.relative_operation_index(to_parent) {
                    self.selected_operation = index;
                    self.keep_selected_operation_visible();
                } else {
                    self.status = "no operation edge in that direction".to_string();
                    return;
                }
            }
            UiPane::Change => {
                if let Some(index) = self.relative_change_index(to_parent) {
                    self.selected_change = index;
                    self.keep_selected_change_visible();
                } else {
                    self.status = "no change edge in that direction".to_string();
                    return;
                }
            }
            _ => {
                self.status =
                    "h/l graph navigation works in Operation and Change panes".to_string();
                return;
            }
        }
        self.sync_after_selection(old_operation, old_change, old_file);
    }

    pub fn relative_operation_index(&self, to_parent: bool) -> Option<usize> {
        let selected = self.operations.get(self.selected_operation)?;
        if to_parent {
            let parent = selected.parent_ids.first()?;
            self.operations.iter().position(|op| &op.id == parent)
        } else {
            self.operations
                .iter()
                .position(|op| op.parent_ids.iter().any(|id| id == &selected.id))
        }
    }

    pub fn relative_change_index(&self, to_parent: bool) -> Option<usize> {
        let selected = self.changes.get(self.selected_change)?;
        if to_parent {
            let parent = selected.parent_commit_ids.first()?;
            self.changes
                .iter()
                .position(|change| &change.commit_id == parent)
        } else {
            self.changes.iter().position(|change| {
                change
                    .parent_commit_ids
                    .iter()
                    .any(|id| id == &selected.commit_id)
            })
        }
    }

    pub fn toggle_pin_comparison(&mut self) {
        let Some(current) = self.selected_change() else {
            self.status = "no change selected to pin".to_string();
            return;
        };
        if let Some(pinned) = self.pinned_change.take() {
            let lines = vec![
                "Pinned comparison".to_string(),
                String::new(),
                format!("pinned:  {}", pinned.brief()),
                format!("current: {}", current.brief()),
            ];
            self.show_overlay("Pinned Comparison", lines);
            self.status = "pin comparison shown".to_string();
        } else {
            self.pinned_change = Some(current.clone());
            self.status = format!("pinned {}", current.brief());
        }
    }

    pub fn extend_file_selection(&mut self, delta: isize) {
        let old_file = self.selected_file;
        let target = move_index(self.selected_file, self.files.len(), delta);
        self.select_file(target, false, true);
        self.keep_selected_file_visible();
        if self.selected_file != old_file {
            self.refresh_diff();
        }
        self.status = format!("{} file(s) selected", self.selected_files.len());
    }

    pub fn select_file(&mut self, index: usize, toggle: bool, extend: bool) {
        if self.files.is_empty() {
            self.selected_file = 0;
            self.selected_files.clear();
            self.file_selection_anchor = None;
            return;
        }
        let index = clamp_index(index, self.files.len());
        self.selected_file = index;
        if toggle {
            if !self.selected_files.insert(index) {
                self.selected_files.remove(&index);
            }
            self.file_selection_anchor = Some(index);
        } else if extend {
            let anchor = self.file_selection_anchor.unwrap_or(index);
            self.selected_files.clear();
            for item in anchor.min(index)..=anchor.max(index) {
                self.selected_files.insert(item);
            }
        } else {
            self.selected_files.clear();
            self.selected_files.insert(index);
            self.file_selection_anchor = Some(index);
        }
    }

    pub fn keep_selected_file_visible(&mut self) {
        keep_selected_visible(
            self.selected_file,
            &mut self.file_scroll,
            self.file_visible_rows,
        );
    }

    pub fn keep_selected_operation_visible(&mut self) {
        keep_selected_visible(
            self.selected_operation,
            &mut self.operation_scroll,
            self.operation_visible_rows,
        );
    }

    pub fn keep_selected_change_visible(&mut self) {
        keep_selected_visible(
            self.selected_change,
            &mut self.change_scroll,
            self.change_visible_rows,
        );
    }

    pub fn scroll_detail_page(&mut self, delta: i16) {
        if delta < 0 {
            self.detail_scroll = self.detail_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as u16);
        }
        self.status = "detail scrolled".to_string();
    }
}
