use ratatui::style::{Color, Modifier, Style};
use ratatui::text::Span;

use super::*;
use crate::graph::{GraphCharset, GraphNode, render_graph_rows};
use crate::tui_support::{bookmark_display, selected_line};

impl TuiApp {
    pub fn context_spans(&self) -> Vec<Span<'_>> {
        let op = self
            .selected_operation_id()
            .map(|id| short(&id).to_string())
            .unwrap_or_else(|| "?".to_string());
        let (label, fg, bg) = self.mode_badge();
        vec![
            Span::styled(
                format!(" {label} "),
                Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "op {op} | current head {} | {} | snapshot only | fs unchanged | gen {}",
                    short(&self.current_head),
                    self.view_mode(),
                    self.observed_at
                ),
                Style::default().fg(Color::Cyan),
            ),
        ]
    }

    pub fn mode_badge(&self) -> (&'static str, Color, Color) {
        if matches!(
            &self.modal,
            ActiveModal::PendingMutation {
                mutation: PendingMutation::ImportHistoricalFile { .. },
                ..
            }
        ) {
            ("IMPORT", Color::Black, Color::Yellow)
        } else if self.mode == UiMode::Surgery {
            ("SURGERY", Color::White, Color::Red)
        } else if self.is_viewing_current_operation() {
            ("CURRENT", Color::Black, Color::Green)
        } else {
            ("TIME TRAVEL", Color::Black, Color::Magenta)
        }
    }

    pub fn status_color(&self) -> Color {
        if matches!(
            &self.modal,
            ActiveModal::PendingMutation {
                mutation: PendingMutation::ImportHistoricalFile { .. },
                ..
            }
        ) {
            Color::Yellow
        } else if self.mode == UiMode::Surgery {
            Color::Red
        } else if self.is_viewing_current_operation() {
            Color::Green
        } else {
            Color::Red
        }
    }

    pub fn operation_title(&self) -> String {
        format!(
            "1 Operation ref:{}",
            self.selected_operation_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string())
        )
    }

    pub fn change_title(&self) -> String {
        format!(
            "2 Change op:{} change:{} commit:{}",
            self.selected_operation_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.changes
                .get(self.selected_change)
                .map(|change| short(&change.change_id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.selected_commit_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string())
        )
    }

    pub fn files_title(&self) -> String {
        format!(
            "3 Files commit:{} file:{}",
            self.selected_commit_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.selected_file_path()
        )
    }

    pub fn operation_labels(&self) -> Vec<String> {
        let charset = GraphCharset::from_env();
        render_graph_rows(
            &self
                .operations
                .iter()
                .map(|operation| GraphNode {
                    id: operation.id.clone(),
                    parent_ids: operation.parent_ids.clone(),
                    marker: if operation.is_current {
                        '@'
                    } else if operation.is_head {
                        match charset {
                            GraphCharset::Unicode => '◆',
                            GraphCharset::Ascii => '*',
                        }
                    } else {
                        match charset {
                            GraphCharset::Unicode => '○',
                            GraphCharset::Ascii => 'o',
                        }
                    },
                    text: operation.label.clone(),
                })
                .collect::<Vec<_>>(),
            charset,
        )
    }

    pub fn change_labels(&self) -> Vec<String> {
        let charset = GraphCharset::from_env();
        render_graph_rows(
            &self
                .changes
                .iter()
                .map(|change| GraphNode {
                    id: change.commit_id.clone(),
                    parent_ids: change.parent_commit_ids.clone(),
                    marker: if change.is_workspace {
                        '@'
                    } else {
                        match charset {
                            GraphCharset::Unicode => '○',
                            GraphCharset::Ascii => 'o',
                        }
                    },
                    text: change.label.clone(),
                })
                .collect::<Vec<_>>(),
            charset,
        )
    }

    pub fn selected_change_detail_lines(&self) -> Vec<String> {
        let Some(change) = self.changes.get(self.selected_change) else {
            return vec!["(nothing selected)".to_string()];
        };
        let mut lines = vec![
            selected_line(&self.change_labels(), self.selected_change),
            format!("change: {}", change.change_id),
            format!("commit: {}", change.commit_id),
        ];
        if !change.parent_commit_ids.is_empty() {
            lines.push(format!(
                "parents: {}",
                change
                    .parent_commit_ids
                    .iter()
                    .map(|id| short(id).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        lines.push(String::new());
        lines.push("Bookmarks:".to_string());
        if change.bookmarks.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            lines.extend(
                change
                    .bookmarks
                    .iter()
                    .map(|bookmark| format!("  {}", bookmark_display(bookmark))),
            );
        }
        lines
    }

    pub fn file_labels(&self) -> Vec<String> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                if self.selected_files.contains(&index) {
                    format!("* {}", file.label)
                } else {
                    format!("  {}", file.label)
                }
            })
            .collect()
    }
}
