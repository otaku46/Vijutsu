use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bookmark {
    pub name: String,
    pub remote: Option<String>,
    #[serde(default)]
    pub is_conflicted: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChangeFlags {
    #[serde(default)]
    pub is_empty: bool,
    #[serde(default)]
    pub has_conflict: bool,
    #[serde(default)]
    pub is_immutable: bool,
    #[serde(default)]
    pub is_hidden: bool,
    #[serde(default)]
    pub is_divergent: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeNode {
    pub change_id: String,
    pub commit_id: String,
    #[serde(default)]
    pub parent_commit_ids: Vec<String>,
    #[serde(default)]
    pub predecessor_commit_ids: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bookmarks: Vec<Bookmark>,
    #[serde(default)]
    pub workspace_names: Vec<String>,
    #[serde(default)]
    pub flags: ChangeFlags,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationNode {
    pub operation_id: String,
    #[serde(default)]
    pub parent_operation_ids: Vec<String>,
    #[serde(default)]
    pub description: String,
    pub timestamp_start: Option<String>,
    pub timestamp_end: Option<String>,
    pub user: Option<String>,
    pub host: Option<String>,
    #[serde(default)]
    pub is_head: bool,
    #[serde(default)]
    pub is_selected: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ChangeGraph {
    pub nodes: Vec<ChangeNode>,
    pub root_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct OperationGraph {
    pub nodes: Vec<OperationNode>,
    pub head_ids: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RewritePair {
    pub change_id: String,
    pub old_commit_id: String,
    pub new_commit_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ParentChange {
    pub commit_id: String,
    pub old_parents: Vec<String>,
    pub new_parents: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BookmarkMove {
    pub name: String,
    pub old_target: Option<String>,
    pub new_target: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WorkspaceMove {
    pub workspace: String,
    pub old_commit_id: Option<String>,
    pub new_commit_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ConflictTransition {
    pub commit_id: String,
    pub had_conflict: bool,
    pub has_conflict: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OperationDelta {
    pub from_op: String,
    pub to_op: String,
    #[serde(default)]
    pub added_commits: Vec<String>,
    #[serde(default)]
    pub removed_commits: Vec<String>,
    #[serde(default)]
    pub rewritten_changes: Vec<RewritePair>,
    #[serde(default)]
    pub parent_changes: Vec<ParentChange>,
    #[serde(default)]
    pub moved_bookmarks: Vec<BookmarkMove>,
    #[serde(default)]
    pub workspace_moves: Vec<WorkspaceMove>,
    #[serde(default)]
    pub conflict_transitions: Vec<ConflictTransition>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum ObservationFreshness {
    CurrentSnapshot,
    FilesystemAhead { changed_paths_known: bool },
    OperationHeadChanged,
    Historical { operation_id: String },
    StaleWorkingCopy,
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ObservationToken {
    pub operation_id: String,
    pub head_set_hash: String,
    pub observed_at_ms: u128,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RepositoryObservation {
    pub selected_operation_id: String,
    pub operation_heads: Vec<String>,
    pub operation_graph: OperationGraph,
    pub change_graph: ChangeGraph,
    pub freshness: ObservationFreshness,
    pub token: ObservationToken,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct JjCapabilities {
    pub version: String,
    pub json_template_function: bool,
    pub escape_json_method: bool,
    pub operation_parent_template: bool,
    pub commit_predecessor_template: bool,
    pub op_diff: bool,
    pub op_show: bool,
    pub op_revert: bool,
    pub op_restore: bool,
    pub op_restore_what: bool,
    pub redo: bool,
    pub util_snapshot: bool,
    pub no_integrate_operation: bool,
}
