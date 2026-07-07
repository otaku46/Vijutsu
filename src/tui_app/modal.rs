use super::*;
use crate::tui_support::{pane_mode_help, run_self, selected_line};

impl TuiApp {
    pub fn show_overlay(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.modal = ActiveModal::Overlay(OverlayPane {
            title: title.into(),
            lines,
            scroll: 0,
        });
    }

    pub fn clear_overlay(&mut self) {
        if matches!(self.modal, ActiveModal::Overlay(_)) {
            self.modal = ActiveModal::None;
        }
    }

    pub fn scroll_overlay_page(&mut self, delta: i16) -> bool {
        let scroll = match &mut self.modal {
            ActiveModal::Overlay(overlay) => &mut overlay.scroll,
            ActiveModal::Describe { scroll, .. }
            | ActiveModal::Commit { scroll, .. }
            | ActiveModal::Bookmark { scroll, .. }
            | ActiveModal::PendingMutation { scroll, .. }
            | ActiveModal::Rebase { scroll, .. } => scroll,
            ActiveModal::None => return false,
        };
        if delta < 0 {
            *scroll = scroll.saturating_sub(delta.unsigned_abs());
        } else {
            *scroll = scroll.saturating_add(delta as u16);
        }
        self.status = "overlay scrolled".to_string();
        true
    }

    pub fn modal_overlay(&self) -> Option<(String, Vec<String>, u16)> {
        match &self.modal {
            ActiveModal::Overlay(overlay) => {
                Some((overlay.title.clone(), overlay.lines.clone(), overlay.scroll))
            }
            ActiveModal::Describe { input, scroll } => Some((
                format!("Describe {}", short(&input.commit_id)),
                vec![
                    "Type new description.".to_string(),
                    "Enter: apply, Esc/N: cancel, Backspace: delete".to_string(),
                    String::new(),
                    input.message.clone(),
                ],
                *scroll,
            )),
            ActiveModal::Commit { input, scroll } => Some((
                "Commit".to_string(),
                vec![
                    "Type commit description.".to_string(),
                    "Enter: confirm, Esc/N: cancel, Backspace: delete".to_string(),
                    String::new(),
                    input.message.clone(),
                ],
                *scroll,
            )),
            ActiveModal::Bookmark { input, scroll } => Some((
                "Bookmark".to_string(),
                vec![
                    "Type local bookmark name.".to_string(),
                    "Enter: confirm, Esc/N: cancel, Backspace: delete".to_string(),
                    String::new(),
                    input.name.clone(),
                ],
                *scroll,
            )),
            ActiveModal::PendingMutation { mutation, scroll } => {
                let mut lines = self.operation_context_lines();
                lines.push(String::new());
                lines.extend(mutation.preview_lines());
                Some((format!("Confirm {}", mutation.name()), lines, *scroll))
            }
            ActiveModal::Rebase { source, scroll } => Some((
                "Rebase source".to_string(),
                vec![
                    format!("source: {}", source.brief()),
                    "Move the cursor to the destination change and press Enter.".to_string(),
                    "Esc clears rebase selection.".to_string(),
                ],
                *scroll,
            )),
            ActiveModal::None => None,
        }
    }

    pub fn has_modal(&self) -> bool {
        !matches!(self.modal, ActiveModal::None)
    }

    pub fn open_focused(&mut self) {
        let (title, lines) = match self.pane {
            UiPane::Operation | UiPane::Repository => {
                let mut lines = self.operation_context_lines();
                lines.push(String::new());
                lines.push(selected_line(
                    &self.operation_labels(),
                    self.selected_operation,
                ));
                lines.push(String::new());
                lines.push(
                    "Enter shows operation details only; it never changes mutation target."
                        .to_string(),
                );
                if !self.is_viewing_current_operation() {
                    lines.push("Mutations are disabled in historical views.".to_string());
                }
                ("4 Operation", lines)
            }
            UiPane::Change => ("4 Change", self.selected_change_detail_lines()),
            UiPane::Files => {
                self.clear_overlay();
                self.refresh_diff();
                self.pane = UiPane::Diff;
                self.set_browse_status("opened file diff");
                return;
            }
            UiPane::Diff => {
                self.scroll_detail_page(12);
                return;
            }
        };
        self.show_overlay(title.trim_start_matches("4 "), lines);
        self.set_browse_status(format!("opened {title} detail"));
    }

    pub fn set_preview(&mut self, title: &str, args: &[&str]) {
        let mut lines = run_self(args)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push("(empty preview)".to_string());
        }
        self.show_overlay(title, lines);
        self.status = format!("rendered {title}");
    }

    pub fn show_help(&mut self) {
        self.show_overlay("Help", pane_mode_help());
        self.status = "help".to_string();
    }

    pub fn show_command_log(&mut self) {
        let mut lines = vec!["Recent commands".to_string(), String::new()];
        if self.command_log.is_empty() {
            lines.push("(no commands yet)".to_string());
        } else {
            lines.extend(self.command_log.iter().rev().map(|entry| {
                let exit = entry
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "-".to_string());
                format!("exit {exit} | {} | {}", entry.status, entry.command)
            }));
        }
        self.show_overlay("Command Log", lines);
        self.status = "command log".to_string();
    }

    pub fn push_command_log(&mut self, command: String, exit_code: Option<i32>, status: String) {
        self.command_log.push(CommandLogEntry {
            command,
            exit_code,
            status,
        });
        const MAX_COMMAND_LOG: usize = 30;
        if self.command_log.len() > MAX_COMMAND_LOG {
            self.command_log.remove(0);
        }
    }

    pub fn input_cursor(&self) -> Option<InputCursor> {
        match &self.modal {
            ActiveModal::Describe { input, .. } => Some(InputCursor {
                line: 3,
                column: input.message.chars().count() as u16,
            }),
            ActiveModal::Commit { input, .. } => Some(InputCursor {
                line: 3,
                column: input.message.chars().count() as u16,
            }),
            ActiveModal::Bookmark { input, .. } => Some(InputCursor {
                line: 3,
                column: input.name.chars().count() as u16,
            }),
            _ => None,
        }
    }
}
