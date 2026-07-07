use std::process::Command;

use crossterm::event::KeyModifiers;
use ratatui::layout::{Constraint, Direction, Layout, Rect};

use crate::tui::UiPane;

pub fn command_result_lines(result: &crate::cli::CommandResult) -> Vec<String> {
    let mut lines = vec![
        format!("command: {}", result.command_display),
        format!("exit: {}", result.exit_code),
    ];
    if !result.stdout.trim().is_empty() {
        lines.push("stdout:".to_string());
        lines.extend(result.stdout.trim().lines().map(str::to_string));
    }
    if !result.stderr.trim().is_empty() {
        lines.push("stderr:".to_string());
        lines.extend(result.stderr.trim().lines().map(str::to_string));
    }
    lines
}

pub fn pane_mode_help() -> Vec<String> {
    vec![
        "Pane x Mode".to_string(),
        String::new(),
        "Pane        Inspect / Current                         Majutsu / Historical                  Shujutsu / Surgery".to_string(),
        "Operation   h/l graph, Enter detail, S surgery         read-only detail, import via Files    a preview op abandon, Esc browse".to_string(),
        "Change      e n d a s b Z, Space pin                   read-only; mutation blocked           no change mutation".to_string(),
        "Files       Enter diff, O open, range select           I import historical file              no file mutation".to_string(),
        "Diff        j/k scroll, Enter page, Shift+Enter up     same                                same".to_string(),
        String::new(),
        "Global      Tab/S-Tab pane, 1-4 focus, z maximize, L command log, ? help, q quit".to_string(),
    ]
}

pub fn non_empty_or(lines: Vec<String>, fallback: &str) -> Vec<String> {
    if lines.is_empty() {
        vec![fallback.to_string()]
    } else {
        lines
    }
}

pub fn selected_line(lines: &[String], index: usize) -> String {
    lines
        .get(index)
        .cloned()
        .unwrap_or_else(|| "(nothing selected)".to_string())
}

pub fn bookmark_label_suffix(bookmarks: &[String]) -> String {
    if bookmarks.is_empty() {
        String::new()
    } else {
        format!(
            " [{}]",
            bookmarks
                .iter()
                .map(|bookmark| bookmark_display(bookmark))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

pub fn bookmark_display(bookmark: &str) -> String {
    if bookmark.contains('@') {
        format!("{bookmark}:remote")
    } else {
        format!("{bookmark}:local")
    }
}

pub fn operation_semantic_suffix(description: &str) -> Option<String> {
    let (label, rest) = [
        ("undo: restore to operation ", "undo target"),
        ("restore to operation ", "state-from"),
        ("restore operation ", "state-from"),
        ("revert operation ", "target"),
        ("undo operation ", "target"),
    ]
    .into_iter()
    .find_map(|(prefix, label)| description.strip_prefix(prefix).map(|rest| (label, rest)))?;
    let target = rest.split_whitespace().next()?;
    Some(format!(" [{label}: {}]", short(target)))
}

pub fn non_empty_join(values: &[String], fallback: &str) -> String {
    if values.is_empty() {
        fallback.to_string()
    } else {
        values.join(", ")
    }
}

pub fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

pub fn move_index(index: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

pub fn keep_selected_visible(selected: usize, scroll: &mut usize, visible_rows: usize) {
    let visible_rows = visible_rows.max(1);
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible_rows {
        *scroll = selected + 1 - visible_rows;
    }
}

pub fn is_command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META)
}

pub fn pane_at(
    column: u16,
    row: u16,
    width: u16,
    height: u16,
    maximized_pane: Option<UiPane>,
) -> Option<UiPane> {
    let body_height = height.saturating_sub(6);
    if row >= body_height {
        return None;
    }
    if let Some(pane) = maximized_pane {
        return Some(pane);
    }
    if column < width.saturating_mul(28) / 100 {
        Some(UiPane::Operation)
    } else if column < width.saturating_mul(56) / 100 {
        if row < body_height.saturating_mul(55) / 100 {
            Some(UiPane::Change)
        } else {
            Some(UiPane::Files)
        }
    } else {
        Some(UiPane::Diff)
    }
}

pub fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

pub fn is_root_commit(commit_id: &str) -> bool {
    commit_id.chars().all(|ch| ch == '0')
}

pub fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

pub fn run_self(args: &[&str]) -> String {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return format!("failed to find current executable: {error}"),
    };
    match Command::new(exe).args(args).output() {
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            if text.trim().is_empty() {
                format!("command exited with {}", output.status)
            } else {
                text
            }
        }
        Err(error) => format!("failed to run vijutsu {}: {error}", args.join(" ")),
    }
}

pub fn resolve_operation_prefix(requested: &str, known_ids: &[&str]) -> Result<String, String> {
    let matches = known_ids
        .iter()
        .filter(|id| id.starts_with(requested))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(requested.to_string()),
        1 => Ok((*matches[0]).to_string()),
        _ => Err(format!("ambiguous operation prefix: {requested}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_prefix_resolves_known_unique_id() {
        assert_eq!(
            resolve_operation_prefix("abc", &["abcdef", "123456"]).unwrap(),
            "abcdef"
        );
        assert_eq!(
            resolve_operation_prefix("missing", &["abcdef"]).unwrap(),
            "missing"
        );
        assert!(resolve_operation_prefix("a", &["abcdef", "axyz"]).is_err());
    }

    #[test]
    fn operation_semantic_suffix_shows_references_without_graph_edges() {
        assert_eq!(
            operation_semantic_suffix(
                "undo: restore to operation abcdef1234567890 trailing ignored"
            )
            .as_deref(),
            Some(" [undo target: abcdef12]")
        );
        assert_eq!(
            operation_semantic_suffix("restore to operation fedcba9876543210").as_deref(),
            Some(" [state-from: fedcba98]")
        );
        assert_eq!(
            operation_semantic_suffix("revert operation 1234567890abcdef").as_deref(),
            Some(" [target: 12345678]")
        );
        assert!(operation_semantic_suffix("abandon commit abcdef").is_none());
    }

    #[test]
    fn bookmark_label_suffix_marks_local_and_remote() {
        assert_eq!(
            bookmark_label_suffix(&["feature".to_string(), "main@origin".to_string()]),
            " [feature:local main@origin:remote]"
        );
    }
}
