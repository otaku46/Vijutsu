use std::process::Command;

use crossterm::event::KeyModifiers;
use ratatui::layout::Rect;

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
    [
        "Operation pane",
        "  j/k move    h/l graph parent/child",
        "  Enter show operation detail",
        "  S surgery mode (a preview abandon, Esc back)",
        "  historical op: read-only, import via Files",
        "",
        "Change pane",
        "  e edit    n new child    d describe",
        "  a abandon    s squash    b rebase",
        "  Z restore    Space pin/compare",
        "  historical view: mutations blocked",
        "",
        "Files pane",
        "  Enter diff    O open in editor",
        "  Shift+Up/Down range    Cmd+click toggle",
        "  I import historical file into @",
        "",
        "Diff pane",
        "  j/k scroll    Enter page    Shift+Enter page up",
        "",
        "Global",
        "  Tab / Shift+Tab switch pane    1-4 focus pane",
        "  r refresh    R snapshot    z maximize",
        "  B bookmark    W add workspace    p push",
        "  L command log    ? this help    q quit",
    ]
    .iter()
    .map(|line| line.to_string())
    .collect()
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

pub fn centered_content_rect(title: &str, lines: &[String], area: Rect) -> Rect {
    let content_width = lines
        .iter()
        .map(|line| line.chars().count())
        .max()
        .unwrap_or(0)
        .max(title.chars().count() + 2) as u16;
    let content_height = lines.len() as u16;
    let width = (content_width + 4).clamp(30, area.width.saturating_sub(4).max(30));
    let height = (content_height + 2).clamp(5, area.height.saturating_sub(2).max(5));
    Rect {
        x: area.x + (area.width.saturating_sub(width)) / 2,
        y: area.y + (area.height.saturating_sub(height)) / 2,
        width: width.min(area.width),
        height: height.min(area.height),
    }
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
