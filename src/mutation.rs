use crate::cli::{CommandEnvelope, CommandResult, execute, isolated_jj};
use crate::core::{ChangeGraph, OperationDelta};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UndoPreview {
    pub target_operation: String,
    pub inverse_delta: OperationDelta,
    pub new_operation_will_be_added: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RedoPreview {
    pub is_available: bool,
    pub unavailable_reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RestorePreview {
    pub target_operation: String,
    pub current_graph: ChangeGraph,
    pub target_graph: ChangeGraph,
    pub delta: OperationDelta,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RevertPreview {
    pub target_operation: String,
    pub inverse_effect: OperationDelta,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbandonPreview {
    pub target_operations: Vec<String>,
    pub target_count: usize,
    pub includes_current_operation: bool,
    pub history_changes: bool,
    pub current_view_changes: bool,
    pub objects_become_unreachable: bool,
    pub immediate_deletion: bool,
    pub gc_deletion_possible: bool,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SnapshotRefreshResult {
    pub new_operation_created: bool,
    pub new_operation_id: Option<String>,
    pub pre_warning_shown: bool,
}

pub fn restore_warnings() -> Vec<String> {
    vec![
        "Later operations will not be deleted.".to_string(),
        "The working copy may be rewritten.".to_string(),
        "A new restore operation will be created.".to_string(),
    ]
}

pub fn snapshot_refresh_warnings() -> Vec<String> {
    vec!["This may snapshot the working copy and create a new operation.".to_string()]
}

pub fn snapshot_refresh_result(pre_warning_shown: bool) -> SnapshotRefreshResult {
    SnapshotRefreshResult {
        new_operation_created: false,
        new_operation_id: None,
        pre_warning_shown,
    }
}

pub fn undo_warnings() -> Vec<String> {
    vec![
        "Undo applies the inverse of the current operation.".to_string(),
        "A new undo operation will be created.".to_string(),
        "The working copy may be rewritten.".to_string(),
    ]
}

pub fn redo_preview(is_available: bool) -> RedoPreview {
    RedoPreview {
        is_available,
        unavailable_reason: (!is_available)
            .then(|| "Redo is not supported by this Jujutsu version.".to_string()),
    }
}

pub fn abandon_preview(
    target_operations: Vec<String>,
    current_operation: Option<&str>,
) -> AbandonPreview {
    let includes_current_operation = current_operation
        .map(|current| {
            target_operations
                .iter()
                .any(|target| current == target || current.starts_with(target))
        })
        .unwrap_or(false);
    let target_count = target_operations.len();
    let mut warnings = vec![
        "Objects may become unreachable after this operation.".to_string(),
        "Recoverability is reduced for abandoned operation history.".to_string(),
        "Objects may be lost after repository garbage collection.".to_string(),
        "No immediate deletion is performed by the preview.".to_string(),
    ];
    if includes_current_operation {
        warnings
            .push("direct execution blocked: target includes the current operation.".to_string());
    }

    AbandonPreview {
        target_operations,
        target_count,
        includes_current_operation,
        history_changes: true,
        current_view_changes: includes_current_operation,
        objects_become_unreachable: true,
        immediate_deletion: false,
        gc_deletion_possible: true,
        warnings,
    }
}

pub fn mutation_command(args: impl IntoIterator<Item = impl Into<String>>) -> CommandEnvelope {
    isolated_jj(args)
}

pub fn execute_mutation(
    args: impl IntoIterator<Item = impl Into<String>>,
) -> std::io::Result<CommandResult> {
    execute(&mutation_command(args))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mutation_command_keeps_jj_isolation() {
        let envelope = mutation_command(["op", "restore", "abc"]);

        assert_eq!(envelope.binary_path, "jj");
        assert!(envelope.args.contains(&"--color=never".to_string()));
        assert!(envelope.args.contains(&"--no-pager".to_string()));
        assert!(envelope.args.contains(&"--quiet".to_string()));
        assert!(envelope.args.ends_with(&[
            "op".to_string(),
            "restore".to_string(),
            "abc".to_string()
        ]));
        assert_eq!(envelope.env, vec![("JJ_CONFIG".to_string(), String::new())]);
    }

    #[test]
    fn abandon_preview_warns_when_target_includes_current_operation() {
        let preview = abandon_preview(vec!["abcdef".to_string()], Some("abcdef123456"));

        assert!(preview.includes_current_operation);
        assert_eq!(preview.target_count, 1);
        assert!(preview.current_view_changes);
        assert!(
            preview
                .warnings
                .iter()
                .any(|warning| warning.contains("direct execution blocked"))
        );
    }
}
