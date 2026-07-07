use crate::state::CommandKind;
use crate::tui::PreviewPanel;
use crate::tui_data::SelectedChange;
use crate::tui_support::{non_empty_join, short};

#[derive(Debug, Clone)]
pub enum PendingMutation {
    Undo,
    Edit {
        change: SelectedChange,
    },
    NewChild {
        change: SelectedChange,
    },
    SquashCurrent {
        source: SelectedChange,
        destination: SelectedChange,
    },
    Abandon {
        change: SelectedChange,
        descendants: Vec<String>,
        bookmarks: Vec<String>,
    },
    Rebase {
        source: SelectedChange,
        destination: SelectedChange,
    },
    Restore {
        change: SelectedChange,
    },
    ImportHistoricalFile {
        source_op: String,
        source: SelectedChange,
        paths: Vec<String>,
    },
    Commit {
        message: String,
    },
    BookmarkSet {
        name: String,
        change: SelectedChange,
    },
    Push {
        bookmark: Option<String>,
    },
}

#[derive(Debug, Clone)]
pub struct MutationPreview {
    panel: PreviewPanel,
    source_target: Vec<String>,
    command: String,
}

impl PendingMutation {
    pub fn name(&self) -> &'static str {
        match self {
            PendingMutation::Undo => "undo last operation",
            PendingMutation::Edit { .. } => "edit selected change",
            PendingMutation::NewChild { .. } => "new child from selected change",
            PendingMutation::SquashCurrent { .. } => "squash current into parent",
            PendingMutation::Abandon { .. } => "abandon selected change",
            PendingMutation::Rebase { .. } => "rebase selected change",
            PendingMutation::Restore { .. } => "restore selected change",
            PendingMutation::ImportHistoricalFile { .. } => {
                "import historical file into current working copy"
            }
            PendingMutation::Commit { .. } => "commit current change",
            PendingMutation::BookmarkSet { .. } => "set bookmark on selected change",
            PendingMutation::Push { .. } => "push to git remote",
        }
    }

    pub fn command_args(&self) -> Vec<String> {
        match self {
            PendingMutation::Undo => vec!["undo".to_string()],
            PendingMutation::Edit { change } => vec!["edit".to_string(), change.commit_id.clone()],
            PendingMutation::NewChild { change } => {
                vec!["new".to_string(), change.commit_id.clone()]
            }
            PendingMutation::SquashCurrent { source, .. } => {
                vec![
                    "squash".to_string(),
                    "-r".to_string(),
                    source.commit_id.clone(),
                ]
            }
            PendingMutation::Abandon { change, .. } => {
                vec!["abandon".to_string(), change.commit_id.clone()]
            }
            PendingMutation::Rebase {
                source,
                destination,
            } => vec![
                "rebase".to_string(),
                "-r".to_string(),
                source.commit_id.clone(),
                "-o".to_string(),
                destination.commit_id.clone(),
            ],
            PendingMutation::Restore { change } => {
                vec![
                    "restore".to_string(),
                    "--changes-in".to_string(),
                    change.commit_id.clone(),
                ]
            }
            PendingMutation::ImportHistoricalFile { source, paths, .. } => {
                let mut args = vec![
                    "restore".to_string(),
                    "--from".to_string(),
                    source.commit_id.clone(),
                    "--into".to_string(),
                    "@".to_string(),
                    "--".to_string(),
                ];
                args.extend(paths.clone());
                args
            }
            PendingMutation::Commit { message } => {
                vec!["commit".to_string(), "-m".to_string(), message.clone()]
            }
            PendingMutation::BookmarkSet { name, change } => vec![
                "bookmark".to_string(),
                "set".to_string(),
                name.clone(),
                "-r".to_string(),
                change.commit_id.clone(),
            ],
            PendingMutation::Push { bookmark } => {
                let mut args = vec!["git".to_string(), "push".to_string()];
                if let Some(bookmark) = bookmark {
                    args.push("--bookmark".to_string());
                    args.push(bookmark.clone());
                }
                args
            }
        }
    }

    pub fn preferred_change_id(&self) -> Option<&str> {
        match self {
            PendingMutation::Edit { change }
            | PendingMutation::NewChild { change }
            | PendingMutation::SquashCurrent { source: change, .. }
            | PendingMutation::Rebase { source: change, .. }
            | PendingMutation::Restore { change }
            | PendingMutation::BookmarkSet { change, .. } => Some(&change.change_id),
            PendingMutation::ImportHistoricalFile { .. } => None,
            PendingMutation::Undo
            | PendingMutation::Abandon { .. }
            | PendingMutation::Commit { .. }
            | PendingMutation::Push { .. } => None,
        }
    }

    pub fn command_kind(&self) -> CommandKind {
        match self {
            PendingMutation::ImportHistoricalFile { .. } => CommandKind::ImportFromHistorical,
            PendingMutation::Undo
            | PendingMutation::Edit { .. }
            | PendingMutation::NewChild { .. }
            | PendingMutation::SquashCurrent { .. }
            | PendingMutation::Abandon { .. }
            | PendingMutation::Rebase { .. }
            | PendingMutation::Restore { .. }
            | PendingMutation::Commit { .. }
            | PendingMutation::BookmarkSet { .. }
            | PendingMutation::Push { .. } => CommandKind::MutateCurrent,
        }
    }

    fn preview(&self) -> MutationPreview {
        let args = self.command_args();
        let command = format!("jj {}", args.join(" "));
        match self {
            PendingMutation::Undo => MutationPreview::new(
                "Undo last operation",
                vec!["target: latest operation".to_string()],
                "restore repository state from before that operation",
                "+1 undo operation",
                "rewritten to the restored state",
                "unchanged",
                "unchanged",
                "the undo operation can itself be undone",
                command,
            ),
            PendingMutation::Edit { change } => MutationPreview::new(
                format!("Edit {}", change.brief()),
                vec![format!("selected: {}", change.brief())],
                "selected change becomes the working-copy change (@)\ndescendants may be rebased as you edit it",
                "+1 edit operation",
                "moves to selected change",
                "unchanged",
                "unchanged",
                "use U to undo the edit operation",
                command,
            ),
            PendingMutation::NewChild { change } => MutationPreview::new(
                format!("New child from {}", change.brief()),
                vec![format!("parent: {}", change.brief())],
                "a new child change is created from the selected change",
                "+1 new operation",
                "moves to the new child change",
                "unchanged",
                "unchanged",
                "use U to undo the new operation",
                command,
            ),
            PendingMutation::SquashCurrent {
                source,
                destination,
            } => MutationPreview::new(
                "Squash current into parent",
                vec![
                    format!("source: {}", source.brief()),
                    format!("destination: {}", destination.brief()),
                ],
                "Before             After\n\n@ C                @ empty\n│                  │\n○ B                ○ B+C",
                "+1 squash operation",
                "current change becomes empty after its diff moves to parent",
                "unchanged",
                "unchanged",
                "old commits remain recoverable through operation history until GC",
                command,
            ),
            PendingMutation::Abandon {
                change,
                descendants,
                bookmarks,
            } => MutationPreview::new(
                format!("Abandon {}", change.brief()),
                vec![
                    format!("abandoned: {}", change.brief()),
                    format!(
                        "descendants rebased: {}",
                        non_empty_join(descendants, "(none)")
                    ),
                    format!(
                        "bookmarks affected: {}",
                        non_empty_join(bookmarks, "(none)")
                    ),
                ],
                "Before             After\n\n@ C                @ C'\n│                  │\n○ B       ->       ○ A\n│\n○ A",
                "+1 abandon operation",
                "rewritten if @ or descendants are rebased",
                "bookmarks may move if they point into abandoned history",
                "unchanged",
                "undo available: y\nabandoned commits remain hidden until GC",
                command,
            ),
            PendingMutation::Rebase {
                source,
                destination,
            } => MutationPreview::new(
                "Rebase",
                vec![
                    format!("source: {}", source.brief()),
                    format!("destination: {}", destination.brief()),
                ],
                "selected revision is moved onto the destination\ndescendants are not moved by this command",
                "+1 rebase operation",
                "rewritten if the working-copy change is affected",
                "unchanged unless bookmarks track rebased commits",
                "unchanged",
                "use U to undo the rebase operation",
                command,
            ),
            PendingMutation::Restore { change } => MutationPreview::new(
                format!("Restore changes in {}", change.brief()),
                vec![format!("changes-in: {}", change.brief())],
                "selected change's diff is undone in the working copy",
                "+1 restore operation",
                "rewritten by the inverse of the selected diff",
                "unchanged",
                "unchanged",
                "use U to undo the restore operation",
                command,
            ),
            PendingMutation::ImportHistoricalFile {
                source_op,
                source,
                paths,
            } => MutationPreview::new(
                "Import historical file into current working copy",
                vec![
                    format!("source op: {}", short(source_op)),
                    format!("source commit: {}", short(&source.commit_id)),
                    format!("source paths: {}", paths.join(", ")),
                    "write target: current operation head (@)".to_string(),
                ],
                "historical change graph remains read-only",
                "+1 restore operation on current head",
                "selected historical file content is written into @",
                "unchanged",
                "unchanged",
                "source commit must remain available until GC\nuse U to undo the import operation",
                command,
            ),
            PendingMutation::Commit { message } => MutationPreview::new(
                "Commit current working-copy change",
                vec![format!("message: {message}")],
                "current change description is updated\na new child change is created",
                "+1 commit operation",
                "moves to the new child change",
                "unchanged",
                "unchanged",
                "use U to undo the commit operation",
                command,
            ),
            PendingMutation::BookmarkSet { name, change } => MutationPreview::new(
                "Set bookmark",
                vec![
                    format!("bookmark: {name}"),
                    format!("target: {}", change.brief()),
                ],
                "change graph is unchanged",
                "+1 bookmark operation",
                "unchanged",
                "local bookmark moves",
                "remote tracking unchanged until push",
                "use U to undo the bookmark move",
                command,
            ),
            PendingMutation::Push { bookmark } => {
                let mut source_target = vec!["remote/bookmark: repository default".to_string()];
                if let Some(bookmark) = bookmark {
                    source_target = vec![format!("bookmark: {bookmark}")];
                }
                MutationPreview::new(
                    "Push to Git remote",
                    source_target,
                    "local change graph is unchanged",
                    "no local jj operation is expected",
                    "unchanged",
                    "local bookmarks unchanged",
                    "remote bookmark may move",
                    "remote bookmark movement depends on remote policy\nlocal operation undo does not necessarily undo a remote push",
                    command,
                )
            }
        }
    }

    pub fn preview_lines(&self) -> Vec<String> {
        self.preview().lines()
    }
}

impl MutationPreview {
    fn new(
        title: impl Into<String>,
        source_target: Vec<String>,
        change_graph_effect: impl Into<String>,
        operation_graph_effect: impl Into<String>,
        working_copy_effect: impl Into<String>,
        bookmark_effect: impl Into<String>,
        remote_tracking_effect: impl Into<String>,
        recoverability_effect: impl Into<String>,
        command: String,
    ) -> Self {
        Self {
            panel: PreviewPanel {
                title: title.into(),
                change_graph_effect: change_graph_effect.into(),
                operation_graph_effect: operation_graph_effect.into(),
                working_copy_effect: working_copy_effect.into(),
                bookmark_effect: bookmark_effect.into(),
                remote_tracking_effect: remote_tracking_effect.into(),
                recoverability_effect: recoverability_effect.into(),
            },
            source_target,
            command,
        }
    }

    fn lines(&self) -> Vec<String> {
        let mut lines = vec![self.panel.title.clone(), String::new()];
        push_section(&mut lines, "Target", &self.source_target);
        push_section_text(&mut lines, "Change graph", &self.panel.change_graph_effect);
        push_section_text(
            &mut lines,
            "Operation graph",
            &self.panel.operation_graph_effect,
        );
        push_section_text(&mut lines, "Working copy", &self.panel.working_copy_effect);
        push_section_text(&mut lines, "Bookmarks", &self.panel.bookmark_effect);
        push_section_text(
            &mut lines,
            "Remote tracking",
            &self.panel.remote_tracking_effect,
        );
        push_section_text(
            &mut lines,
            "Recoverability",
            &self.panel.recoverability_effect,
        );
        push_section(&mut lines, "Command", std::slice::from_ref(&self.command));
        lines.push(String::new());
        lines.push("Press y to execute, Esc or N to cancel.".to_string());
        lines
    }
}

fn push_section(lines: &mut Vec<String>, title: &str, values: &[String]) {
    lines.push(format!("{title}:"));
    for value in values {
        if value.is_empty() {
            lines.push(String::new());
        } else {
            lines.push(format!("  {value}"));
        }
    }
    lines.push(String::new());
}

fn push_section_text(lines: &mut Vec<String>, title: &str, value: &str) {
    let values = value.lines().map(str::to_string).collect::<Vec<_>>();
    push_section(lines, title, &values);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn push_selected_bookmark_uses_bookmark_arg() {
        let mutation = PendingMutation::Push {
            bookmark: Some("feature".to_string()),
        };

        assert_eq!(
            mutation.command_args(),
            vec![
                "git".to_string(),
                "push".to_string(),
                "--bookmark".to_string(),
                "feature".to_string(),
            ]
        );
    }

    #[test]
    fn bookmark_set_uses_selected_change_revision() {
        let mutation = PendingMutation::BookmarkSet {
            name: "feature".to_string(),
            change: SelectedChange {
                change_id: "change-id".to_string(),
                commit_id: "commit-id".to_string(),
            },
        };

        assert_eq!(
            mutation.command_args(),
            vec![
                "bookmark".to_string(),
                "set".to_string(),
                "feature".to_string(),
                "-r".to_string(),
                "commit-id".to_string(),
            ]
        );
    }

    #[test]
    fn mutation_preview_always_shows_effect_sections() {
        let change = SelectedChange {
            change_id: "change-id".to_string(),
            commit_id: "commit-id".to_string(),
        };
        let mutations = vec![
            PendingMutation::Undo,
            PendingMutation::Edit {
                change: change.clone(),
            },
            PendingMutation::NewChild {
                change: change.clone(),
            },
            PendingMutation::SquashCurrent {
                source: change.clone(),
                destination: change.clone(),
            },
            PendingMutation::Abandon {
                change: change.clone(),
                descendants: vec!["child".to_string()],
                bookmarks: vec!["feature".to_string()],
            },
            PendingMutation::Rebase {
                source: change.clone(),
                destination: change.clone(),
            },
            PendingMutation::Restore {
                change: change.clone(),
            },
            PendingMutation::ImportHistoricalFile {
                source_op: "op-old".to_string(),
                source: change.clone(),
                paths: vec!["rireki.txt".to_string()],
            },
            PendingMutation::Commit {
                message: "message".to_string(),
            },
            PendingMutation::BookmarkSet {
                name: "feature".to_string(),
                change,
            },
            PendingMutation::Push {
                bookmark: Some("feature".to_string()),
            },
        ];

        for mutation in mutations {
            let lines = mutation.preview_lines();
            for section in [
                "Target:",
                "Change graph:",
                "Operation graph:",
                "Working copy:",
                "Recoverability:",
                "Command:",
            ] {
                assert!(
                    lines.iter().any(|line| line == section),
                    "{} preview missing {section}",
                    mutation.name()
                );
            }
        }
    }

    #[test]
    fn historical_file_import_restores_source_commit_path_into_current() {
        let mutation = PendingMutation::ImportHistoricalFile {
            source_op: "op-old".to_string(),
            source: SelectedChange {
                change_id: "change-id".to_string(),
                commit_id: "commit-id".to_string(),
            },
            paths: vec!["rireki.txt".to_string(), "note.txt".to_string()],
        };

        assert_eq!(mutation.command_kind(), CommandKind::ImportFromHistorical);
        assert_eq!(
            mutation.command_args(),
            vec![
                "restore".to_string(),
                "--from".to_string(),
                "commit-id".to_string(),
                "--into".to_string(),
                "@".to_string(),
                "--".to_string(),
                "rireki.txt".to_string(),
                "note.txt".to_string(),
            ]
        );
    }
}
