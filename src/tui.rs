use crate::cli::FileEntry;
use crate::core::{ObservationFreshness, OperationDelta, RepositoryObservation};
use crate::mutation::{
    AbandonPreview, RedoPreview, RestorePreview, RevertPreview, SnapshotRefreshResult, UndoPreview,
};
use crate::watcher::MultipleHeadsView;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiPane {
    Repository,
    Operation,
    Change,
    Files,
    Diff,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyAction {
    Char(char),
    Tab,
    Ctrl(char),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiCommand {
    MoveSelection { delta: isize },
    SwitchPane { from: UiPane, to: UiPane },
    RefreshReadOnly,
    PreviewUndo,
    PreviewRedo,
    PreviewSnapshot,
    Quit,
    Noop,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreviewPanel {
    pub title: String,
    pub change_graph_effect: String,
    pub operation_graph_effect: String,
    pub working_copy_effect: String,
    pub bookmark_effect: String,
    pub remote_tracking_effect: String,
    pub recoverability_effect: String,
}

pub struct ChangeDetails {
    pub selected_commit_id: String,
    pub files: Vec<FileEntry>,
    pub diff: String,
}

pub struct DeltaDetails {
    pub delta: OperationDelta,
}

pub fn render_observation(
    observation: &RepositoryObservation,
    details: Option<&ChangeDetails>,
    delta: Option<&DeltaDetails>,
) -> String {
    let mut out = String::new();
    out.push_str("Repository\n");
    out.push_str(&format!("{}\n", freshness_label(&observation.freshness)));
    out.push_str(&format!("heads: {}\n", observation.operation_heads.len()));
    out.push_str("\nOperation graph\n");
    for operation in &observation.operation_graph.nodes {
        let selected = if operation.operation_id == observation.selected_operation_id {
            "@"
        } else {
            " "
        };
        let head = if operation.is_head { "◆" } else { " " };
        out.push_str(&format!(
            "{selected}{head} {} {}{}\n",
            short(&operation.operation_id),
            operation.description,
            operation_metadata(operation)
        ));
    }
    out.push_str("\nChange graph\n");
    for change in &observation.change_graph.nodes {
        let selected = details
            .filter(|details| details.selected_commit_id == change.commit_id)
            .map(|_| "@")
            .unwrap_or(" ");
        out.push_str(&format!(
            "{selected}○ {} {} {}{}\n",
            short(&change.change_id),
            short(&change.commit_id),
            change.description,
            change_metadata(change)
        ));
    }
    if let Some(details) = details {
        out.push_str("\nFiles\n");
        if details.files.is_empty() {
            out.push_str("(no files)\n");
        } else {
            for file in &details.files {
                out.push_str(&format!("{} {}\n", file.status, file.path));
            }
        }

        out.push_str("\nDiff\n");
        out.push_str(&truncate_lines(&details.diff, 80));
    }
    if let Some(delta) = delta {
        out.push_str("\nOperation delta\n");
        out.push_str(&format!(
            "{} -> {}\n",
            short(&delta.delta.from_op),
            short(&delta.delta.to_op)
        ));
        out.push_str(&format!(
            "added commits: {}\n",
            delta.delta.added_commits.len()
        ));
        out.push_str(&format!(
            "removed commits: {}\n",
            delta.delta.removed_commits.len()
        ));
        out.push_str(&format!(
            "rewritten changes: {}\n",
            delta.delta.rewritten_changes.len()
        ));
        out.push_str(&format!(
            "parent changes: {}\n",
            delta.delta.parent_changes.len()
        ));
        out.push_str(&format!(
            "conflict transitions: {}\n",
            delta.delta.conflict_transitions.len()
        ));
        for rewrite in &delta.delta.rewritten_changes {
            out.push_str(&format!(
                "rewrite {} {} -> {}\n",
                short(&rewrite.change_id),
                short(&rewrite.old_commit_id),
                short(&rewrite.new_commit_id)
            ));
        }
    }
    out
}

pub fn preview_panel_for_abandon(preview: &AbandonPreview) -> PreviewPanel {
    PreviewPanel {
        title: format!(
            "Operation abandon {} target(s): {}",
            preview.target_count,
            preview
                .target_operations
                .iter()
                .map(|target| short(target).to_string())
                .collect::<Vec<_>>()
                .join(", ")
        ),
        change_graph_effect: "unchanged".to_string(),
        operation_graph_effect: if preview.history_changes {
            "history changed".to_string()
        } else {
            "unchanged".to_string()
        },
        working_copy_effect: if preview.current_view_changes {
            "may change; target includes current operation".to_string()
        } else {
            "unchanged".to_string()
        },
        bookmark_effect: "unchanged".to_string(),
        remote_tracking_effect: "unknown".to_string(),
        recoverability_effect: if preview.gc_deletion_possible {
            "reduced; may be lost after GC".to_string()
        } else {
            "maintained".to_string()
        },
    }
}

pub fn preview_panel_for_restore(preview: &RestorePreview) -> PreviewPanel {
    PreviewPanel {
        title: format!("Operation restore {}", short(&preview.target_operation)),
        change_graph_effect: delta_effect_label(&preview.delta),
        operation_graph_effect: "new restore operation will be added".to_string(),
        working_copy_effect: "may be rewritten".to_string(),
        bookmark_effect: "may move".to_string(),
        remote_tracking_effect: "unknown".to_string(),
        recoverability_effect: "maintained".to_string(),
    }
}

pub fn preview_panel_for_revert(preview: &RevertPreview) -> PreviewPanel {
    PreviewPanel {
        title: format!("Operation revert {}", short(&preview.target_operation)),
        change_graph_effect: delta_effect_label(&preview.inverse_effect),
        operation_graph_effect: "new revert operation will be added".to_string(),
        working_copy_effect: "may be rewritten".to_string(),
        bookmark_effect: "may move".to_string(),
        remote_tracking_effect: "unknown".to_string(),
        recoverability_effect: "maintained".to_string(),
    }
}

pub fn preview_panel_for_undo(preview: &UndoPreview) -> PreviewPanel {
    PreviewPanel {
        title: format!("Undo to {}", short(&preview.target_operation)),
        change_graph_effect: delta_effect_label(&preview.inverse_delta),
        operation_graph_effect: if preview.new_operation_will_be_added {
            "new undo operation will be added".to_string()
        } else {
            "unchanged".to_string()
        },
        working_copy_effect: "may be rewritten".to_string(),
        bookmark_effect: "may move".to_string(),
        remote_tracking_effect: "unknown".to_string(),
        recoverability_effect: "maintained".to_string(),
    }
}

pub fn preview_panel_for_redo(preview: &RedoPreview) -> PreviewPanel {
    PreviewPanel {
        title: "Redo".to_string(),
        change_graph_effect: if preview.is_available {
            "unknown until executed by jj".to_string()
        } else {
            "unavailable".to_string()
        },
        operation_graph_effect: if preview.is_available {
            "new redo operation will be added".to_string()
        } else {
            "unchanged".to_string()
        },
        working_copy_effect: if preview.is_available {
            "may be rewritten".to_string()
        } else {
            "unchanged".to_string()
        },
        bookmark_effect: "unknown".to_string(),
        remote_tracking_effect: "unknown".to_string(),
        recoverability_effect: "maintained".to_string(),
    }
}

pub fn preview_panel_for_snapshot_refresh(_preview: &SnapshotRefreshResult) -> PreviewPanel {
    PreviewPanel {
        title: "Snapshot refresh".to_string(),
        change_graph_effect: "working-copy commit may be rewritten".to_string(),
        operation_graph_effect: "new snapshot operation may be added".to_string(),
        working_copy_effect: "filesystem changes may be snapshotted".to_string(),
        bookmark_effect: "unchanged".to_string(),
        remote_tracking_effect: "unchanged".to_string(),
        recoverability_effect: "maintained".to_string(),
    }
}

pub fn render_preview_panel(panel: &PreviewPanel, warnings: &[String]) -> String {
    let mut out = String::new();
    out.push_str(&format!("Preview: {}\n", panel.title));
    out.push_str(&format!("Change graph: {}\n", panel.change_graph_effect));
    out.push_str(&format!(
        "Operation graph: {}\n",
        panel.operation_graph_effect
    ));
    out.push_str(&format!("Working copy: {}\n", panel.working_copy_effect));
    out.push_str(&format!("Bookmarks: {}\n", panel.bookmark_effect));
    out.push_str(&format!(
        "Remote tracking: {}\n",
        panel.remote_tracking_effect
    ));
    out.push_str(&format!(
        "Recoverability: {}\n",
        panel.recoverability_effect
    ));
    for warning in warnings {
        out.push_str(&format!("warning: {warning}\n"));
    }
    out
}

pub fn render_multiple_heads(view: &MultipleHeadsView) -> String {
    let mut out = String::new();
    out.push_str(&format!("Operation heads: {}\n", view.head_count));
    for head in &view.head_ids {
        out.push_str(&format!("◆ {}\n", short(head)));
    }
    out.push_str(&view.message);
    out.push('\n');
    out
}

fn delta_effect_label(delta: &OperationDelta) -> String {
    let changed = !delta.added_commits.is_empty()
        || !delta.removed_commits.is_empty()
        || !delta.rewritten_changes.is_empty()
        || !delta.parent_changes.is_empty()
        || !delta.conflict_transitions.is_empty();
    if changed {
        "changes".to_string()
    } else {
        "unchanged".to_string()
    }
}

fn freshness_label(freshness: &ObservationFreshness) -> String {
    match freshness {
        ObservationFreshness::CurrentSnapshot => "Snapshot state: current".to_string(),
        ObservationFreshness::FilesystemAhead { .. } => {
            "Snapshot state: filesystem may have newer changes".to_string()
        }
        ObservationFreshness::OperationHeadChanged => {
            "Snapshot state: operation head changed during observation".to_string()
        }
        ObservationFreshness::Historical { operation_id } => {
            format!(
                "Snapshot state: Historical view; working-copy filesystem is not reflected ({})",
                short(operation_id)
            )
        }
        ObservationFreshness::StaleWorkingCopy => "Snapshot state: stale working copy".to_string(),
        ObservationFreshness::Unknown => "Snapshot state: unknown".to_string(),
    }
}

pub fn handle_key_input(key: KeyAction, selection: UiPane) -> UiCommand {
    match key {
        KeyAction::Char('j') => UiCommand::MoveSelection { delta: 1 },
        KeyAction::Char('k') => UiCommand::MoveSelection { delta: -1 },
        KeyAction::Tab => UiCommand::SwitchPane {
            from: selection,
            to: next_pane(selection),
        },
        KeyAction::Char('r') => UiCommand::RefreshReadOnly,
        KeyAction::Char('R') => UiCommand::PreviewSnapshot,
        KeyAction::Char('u') => UiCommand::PreviewUndo,
        KeyAction::Ctrl('r') | KeyAction::Ctrl('R') => UiCommand::PreviewRedo,
        KeyAction::Char('q') => UiCommand::Quit,
        _ => UiCommand::Noop,
    }
}

pub fn render_keys() -> String {
    [
        "Keys",
        "j: next",
        "k: previous",
        "Tab: switch pane",
        "r: read-only refresh",
        "R: snapshot preview",
        "u: undo preview",
        "Ctrl-r: redo preview",
        "O: open selected file externally",
        "c: commit current change",
        "B: set or move local bookmark to selected change",
        "p: push to git remote",
        "e: edit selected change",
        "n: new child from selected change",
        "d: describe selected change",
        "s: squash current into parent",
        "a: abandon selected change",
        "b: rebase selected change",
        "z: restore selected change",
        "U: undo last operation",
        "?: show this help",
        "q: quit",
    ]
    .join("\n")
        + "\n"
}

fn next_pane(pane: UiPane) -> UiPane {
    match pane {
        UiPane::Repository => UiPane::Operation,
        UiPane::Operation => UiPane::Change,
        UiPane::Change => UiPane::Files,
        UiPane::Files => UiPane::Diff,
        UiPane::Diff => UiPane::Operation,
    }
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn operation_metadata(operation: &crate::core::OperationNode) -> String {
    let mut parts = Vec::new();
    if !operation.parent_operation_ids.is_empty() {
        parts.push(format!(
            "parents:{}",
            operation
                .parent_operation_ids
                .iter()
                .map(|id| short(id).to_string())
                .collect::<Vec<_>>()
                .join(",")
        ));
    }
    if let Some(user) = &operation.user
        && !user.is_empty()
    {
        parts.push(format!("user:{user}"));
    }
    if let Some(host) = &operation.host
        && !host.is_empty()
    {
        parts.push(format!("host:{host}"));
    }
    if let Some(timestamp) = &operation.timestamp_start
        && !timestamp.is_empty()
    {
        parts.push(format!("time:{timestamp}"));
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(" "))
    }
}

fn change_metadata(change: &crate::core::ChangeNode) -> String {
    let mut parts = Vec::new();
    if !change.bookmarks.is_empty() {
        let bookmarks = change
            .bookmarks
            .iter()
            .map(|bookmark| {
                let mut name = bookmark.name.clone();
                if let Some(remote) = &bookmark.remote {
                    name.push('@');
                    name.push_str(remote);
                }
                if bookmark.is_conflicted {
                    name.push('!');
                }
                name
            })
            .collect::<Vec<_>>()
            .join(",");
        parts.push(format!("bookmark:{bookmarks}"));
    }
    if !change.workspace_names.is_empty() {
        parts.push(format!("workspace:{}", change.workspace_names.join(",")));
    }
    if change.flags.is_empty {
        parts.push("empty".to_string());
    }
    if change.flags.has_conflict {
        parts.push("conflict".to_string());
    }
    if change.flags.is_immutable {
        parts.push("immutable".to_string());
    }
    if change.flags.is_hidden {
        parts.push("hidden".to_string());
    }
    if change.flags.is_divergent {
        parts.push("divergent".to_string());
    }

    if parts.is_empty() {
        String::new()
    } else {
        format!(" [{}]", parts.join(" "))
    }
}

fn truncate_lines(text: &str, max_lines: usize) -> String {
    let mut out = String::new();
    for (index, line) in text.lines().enumerate() {
        if index == max_lines {
            out.push_str("... diff truncated ...\n");
            break;
        }
        out.push_str(line);
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use crate::core::{
        Bookmark, ChangeFlags, ChangeGraph, ChangeNode, OperationDelta, OperationGraph,
        OperationNode,
    };
    use crate::mutation::{UndoPreview, abandon_preview};

    use super::*;

    #[test]
    fn abandon_preview_panel_shows_six_effect_axes() {
        let panel = preview_panel_for_abandon(&abandon_preview(vec!["op1".to_string()], None));

        assert_eq!(panel.change_graph_effect, "unchanged");
        assert_eq!(panel.operation_graph_effect, "history changed");
        assert_eq!(panel.working_copy_effect, "unchanged");
        assert_eq!(panel.bookmark_effect, "unchanged");
        assert_eq!(panel.remote_tracking_effect, "unknown");
        assert_eq!(panel.recoverability_effect, "reduced; may be lost after GC");
    }

    #[test]
    fn delta_details_render_core_counts() {
        let rendered = render_observation(
            &RepositoryObservation {
                selected_operation_id: "op2".to_string(),
                operation_heads: Vec::new(),
                operation_graph: crate::core::OperationGraph::default(),
                change_graph: crate::core::ChangeGraph::default(),
                freshness: ObservationFreshness::CurrentSnapshot,
                token: crate::core::ObservationToken {
                    operation_id: "op2".to_string(),
                    head_set_hash: String::new(),
                    observed_at_ms: 0,
                },
            },
            None,
            Some(&DeltaDetails {
                delta: OperationDelta {
                    from_op: "op1".to_string(),
                    to_op: "op2".to_string(),
                    added_commits: vec!["a".to_string()],
                    removed_commits: Vec::new(),
                    rewritten_changes: Vec::new(),
                    parent_changes: Vec::new(),
                    moved_bookmarks: Vec::new(),
                    workspace_moves: Vec::new(),
                    conflict_transitions: Vec::new(),
                },
            }),
        );

        assert!(rendered.contains("Operation delta"));
        assert!(rendered.contains("added commits: 1"));
    }

    #[test]
    fn change_graph_renders_metadata() {
        let rendered = render_observation(
            &RepositoryObservation {
                selected_operation_id: "op1".to_string(),
                operation_heads: Vec::new(),
                operation_graph: crate::core::OperationGraph::default(),
                change_graph: ChangeGraph {
                    nodes: vec![ChangeNode {
                        change_id: "change123".to_string(),
                        commit_id: "commit123".to_string(),
                        parent_commit_ids: Vec::new(),
                        predecessor_commit_ids: Vec::new(),
                        description: "metadata".to_string(),
                        bookmarks: vec![Bookmark {
                            name: "main".to_string(),
                            remote: Some("origin".to_string()),
                            is_conflicted: true,
                        }],
                        workspace_names: vec!["default".to_string()],
                        flags: ChangeFlags {
                            is_empty: true,
                            has_conflict: true,
                            is_immutable: true,
                            is_hidden: true,
                            is_divergent: true,
                        },
                    }],
                    root_ids: Vec::new(),
                },
                freshness: ObservationFreshness::CurrentSnapshot,
                token: crate::core::ObservationToken {
                    operation_id: "op1".to_string(),
                    head_set_hash: String::new(),
                    observed_at_ms: 0,
                },
            },
            None,
            None,
        );

        assert!(rendered.contains("bookmark:main@origin!"));
        assert!(rendered.contains("workspace:default"));
        assert!(rendered.contains("empty conflict immutable hidden divergent"));
    }

    #[test]
    fn operation_graph_renders_metadata() {
        let rendered = render_observation(
            &RepositoryObservation {
                selected_operation_id: "op2".to_string(),
                operation_heads: vec!["op2".to_string()],
                operation_graph: OperationGraph {
                    nodes: vec![OperationNode {
                        operation_id: "op2abcdef".to_string(),
                        parent_operation_ids: vec!["op1abcdef".to_string()],
                        description: "describe".to_string(),
                        timestamp_start: Some("2026-07-03T00:00:00Z".to_string()),
                        timestamp_end: None,
                        user: Some("alice".to_string()),
                        host: Some("host1".to_string()),
                        is_head: true,
                        is_selected: false,
                    }],
                    head_ids: vec!["op2abcdef".to_string()],
                },
                change_graph: ChangeGraph::default(),
                freshness: ObservationFreshness::CurrentSnapshot,
                token: crate::core::ObservationToken {
                    operation_id: "op2".to_string(),
                    head_set_hash: String::new(),
                    observed_at_ms: 0,
                },
            },
            None,
            None,
        );

        assert!(rendered.contains("parents:op1abcde"));
        assert!(rendered.contains("user:alice"));
        assert!(rendered.contains("host:host1"));
        assert!(rendered.contains("time:2026-07-03T00:00:00Z"));
    }

    #[test]
    fn render_preview_panel_shows_six_axes() {
        let panel = preview_panel_for_redo(&RedoPreview {
            is_available: false,
            unavailable_reason: Some("no redo".to_string()),
        });
        let rendered = render_preview_panel(&panel, &["no redo".to_string()]);

        assert!(rendered.contains("Change graph: unavailable"));
        assert!(rendered.contains("Operation graph: unchanged"));
        assert!(rendered.contains("Recoverability: maintained"));
        assert!(rendered.contains("warning: no redo"));
    }

    #[test]
    fn undo_preview_panel_shows_new_operation() {
        let panel = preview_panel_for_undo(&UndoPreview {
            target_operation: "op1abcdef".to_string(),
            inverse_delta: OperationDelta {
                from_op: "op2".to_string(),
                to_op: "op1".to_string(),
                added_commits: Vec::new(),
                removed_commits: vec!["gone".to_string()],
                rewritten_changes: Vec::new(),
                parent_changes: Vec::new(),
                moved_bookmarks: Vec::new(),
                workspace_moves: Vec::new(),
                conflict_transitions: Vec::new(),
            },
            new_operation_will_be_added: true,
            warnings: Vec::new(),
        });

        assert_eq!(panel.change_graph_effect, "changes");
        assert_eq!(
            panel.operation_graph_effect,
            "new undo operation will be added"
        );
    }

    #[test]
    fn snapshot_refresh_preview_warns_about_new_operation() {
        let panel = preview_panel_for_snapshot_refresh(&SnapshotRefreshResult {
            new_operation_created: false,
            new_operation_id: None,
            pre_warning_shown: true,
        });
        let rendered = render_preview_panel(
            &panel,
            &["This may snapshot the working copy and create a new operation.".to_string()],
        );

        assert!(rendered.contains("Preview: Snapshot refresh"));
        assert!(rendered.contains("new snapshot operation may be added"));
    }

    #[test]
    fn render_multiple_heads_lists_each_head() {
        let rendered = render_multiple_heads(&MultipleHeadsView {
            head_count: 2,
            head_ids: vec!["abcdef123".to_string(), "123456789".to_string()],
            message: "No operation was integrated.".to_string(),
            selected_head: None,
        });

        assert!(rendered.contains("Operation heads: 2"));
        assert!(rendered.contains("◆ abcdef12"));
        assert!(rendered.contains("◆ 12345678"));
    }

    #[test]
    fn freshness_labels_distinguish_current_filesystem_and_historical() {
        assert_eq!(
            freshness_label(&ObservationFreshness::CurrentSnapshot),
            "Snapshot state: current"
        );
        assert_eq!(
            freshness_label(&ObservationFreshness::FilesystemAhead {
                changed_paths_known: false
            }),
            "Snapshot state: filesystem may have newer changes"
        );
        assert!(
            freshness_label(&ObservationFreshness::Historical {
                operation_id: "op123456".to_string()
            })
            .contains("Historical view; working-copy filesystem is not reflected")
        );
    }

    #[test]
    fn key_input_maps_to_ui_commands() {
        assert_eq!(
            handle_key_input(KeyAction::Char('j'), UiPane::Repository),
            UiCommand::MoveSelection { delta: 1 }
        );
        assert_eq!(
            handle_key_input(KeyAction::Tab, UiPane::Repository),
            UiCommand::SwitchPane {
                from: UiPane::Repository,
                to: UiPane::Operation
            }
        );
        assert_eq!(
            handle_key_input(KeyAction::Ctrl('r'), UiPane::Diff),
            UiCommand::PreviewRedo
        );
        assert_eq!(
            handle_key_input(KeyAction::Char('q'), UiPane::Diff),
            UiCommand::Quit
        );
    }
}
