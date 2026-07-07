use crate::cli::{ParseResult, fetch_change_records, fetch_operation_records, jj_version};
use crate::observation::operation_graph_from_parse;
use crate::state_fingerprint::{StateFingerprint, state_fingerprint, state_suffixes};
use crate::subcommands::parse_diagnostic;
use crate::tui_support::{bookmark_label_suffix, is_root_commit, operation_semantic_suffix, short};

pub struct TuiOperation {
    pub id: String,
    pub label: String,
    pub parent_ids: Vec<String>,
    pub is_current: bool,
    pub is_head: bool,
}

pub struct TuiChange {
    pub change_id: String,
    pub commit_id: String,
    pub label: String,
    pub parent_commit_ids: Vec<String>,
    pub bookmarks: Vec<String>,
    pub is_workspace: bool,
}

pub struct TuiFile {
    pub path: String,
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct SelectedChange {
    pub change_id: String,
    pub commit_id: String,
}

impl SelectedChange {
    pub fn brief(&self) -> String {
        format!("{} {}", short(&self.change_id), short(&self.commit_id))
    }
}

impl From<&TuiChange> for SelectedChange {
    fn from(change: &TuiChange) -> Self {
        Self {
            change_id: change.change_id.clone(),
            commit_id: change.commit_id.clone(),
        }
    }
}

impl TuiChange {
    pub fn brief(&self) -> String {
        format!("{} {}", short(&self.change_id), short(&self.commit_id))
    }

    pub fn local_bookmark(&self) -> Option<String> {
        self.bookmarks
            .iter()
            .find(|bookmark| !bookmark.contains('@'))
            .cloned()
    }
}

pub fn load_operations() -> Result<(String, String, Vec<TuiOperation>), String> {
    let version_result =
        jj_version().map_err(|error| format!("failed to run jj version: {error}"))?;
    if version_result.exit_code != 0 {
        return Err(format!(
            "jj version failed: {}",
            version_result.stderr.trim()
        ));
    }
    let version = version_result.stdout.trim().to_string();
    let parse = fetch_operation_records(&version, "@", 80)
        .map_err(|error| format!("failed to fetch operations: {error}"))?;
    fail_tui_parse("operation graph", &parse)?;
    let graph = operation_graph_from_parse(parse.clone());
    let current_head = graph
        .head_ids
        .first()
        .cloned()
        .unwrap_or_else(|| "?".to_string());
    let mut operations: Vec<TuiOperation> = Vec::new();
    let mut skipped_snapshots = 0usize;

    for record in parse.records {
        let is_head = graph.head_ids.contains(&record.operation_id);
        let is_snapshot = record.description == "snapshot working copy";
        if is_snapshot
            && operations
                .last()
                .map(|operation| operation.label.contains("snapshot working copy"))
                .unwrap_or(false)
            && !record.is_current
        {
            if let Some(previous) = operations.last_mut() {
                previous.parent_ids = record.parent_operation_ids.clone();
            }
            skipped_snapshots += 1;
            continue;
        }
        if skipped_snapshots > 0 {
            if let Some(previous) = operations.last_mut() {
                previous
                    .label
                    .push_str(&format!(" (+{skipped_snapshots} snapshots)"));
            }
            skipped_snapshots = 0;
        }
        let description = if is_snapshot {
            "snapshot working copy".to_string()
        } else if record.description.is_empty() {
            "(no description)".to_string()
        } else {
            record.description
        };
        let semantic_suffix = operation_semantic_suffix(&description).unwrap_or_default();
        operations.push(TuiOperation {
            id: record.operation_id.clone(),
            label: format!(
                "{} {description}{semantic_suffix}",
                short(&record.operation_id)
            ),
            parent_ids: record.parent_operation_ids,
            is_current: record.is_current,
            is_head,
        });
    }
    if skipped_snapshots > 0
        && let Some(previous) = operations.last_mut()
    {
        previous
            .label
            .push_str(&format!(" (+{skipped_snapshots} snapshots)"));
    }
    annotate_operation_states(&version, &mut operations);

    Ok((version, current_head, operations))
}

pub fn annotate_operation_states(version: &str, operations: &mut [TuiOperation]) {
    // ponytail: each fingerprint spawns jj; make this async/cached before raising the limit.
    const STATE_FINGERPRINT_LIMIT: usize = 16;
    let visible_len = operations.len().min(STATE_FINGERPRINT_LIMIT);
    let (visible_operations, _) = operations.split_at_mut(visible_len);
    let fingerprints = visible_operations
        .iter()
        .map(|operation| operation_state_fingerprint(version, &operation.id))
        .collect::<Vec<_>>();
    let op_ids = visible_operations
        .iter()
        .map(|operation| operation.id.clone())
        .collect::<Vec<_>>();

    for (operation, suffix) in visible_operations
        .iter_mut()
        .zip(state_suffixes(&op_ids, &fingerprints))
    {
        if !suffix.is_empty() {
            operation.label = label_with_state_suffix(&operation.label, &suffix);
        }
    }
}

pub fn label_with_state_suffix(label: &str, suffix: &str) -> String {
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return label.to_string();
    }
    match label.split_once(' ') {
        Some((op_id, rest)) => format!("{op_id} {suffix} {rest}"),
        None => format!("{label} {suffix}"),
    }
}

pub fn operation_state_fingerprint(version: &str, op_id: &str) -> Option<StateFingerprint> {
    let parse = fetch_change_records(version, op_id, "all()").ok()?;
    fail_tui_parse("state fingerprint change graph", &parse).ok()?;
    Some(state_fingerprint(&parse.records))
}

pub fn load_changes(version: &str, op_id: &str) -> Result<Vec<TuiChange>, String> {
    let parse = fetch_change_records(version, op_id, "all()")
        .map_err(|error| format!("failed to fetch changes: {error}"))?;
    fail_tui_parse("change graph", &parse)?;
    Ok(parse
        .records
        .into_iter()
        .map(|record| {
            let root = is_root_commit(&record.commit_id);
            let is_workspace = !record.working_copies.is_empty();
            let label = if root {
                "ROOT [immutable]".to_string()
            } else {
                let mut flags = Vec::new();
                if is_workspace {
                    flags.push("workspace");
                }
                if record.is_empty {
                    flags.push("empty");
                }
                if record.has_conflict {
                    flags.push("conflict");
                }
                if record.is_immutable {
                    flags.push("immutable");
                }
                let bookmarks = bookmark_label_suffix(&record.bookmarks);
                let suffix = if flags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", flags.join(" "))
                };
                format!(
                    "{} {} {}{}{}",
                    short(&record.change_id),
                    short(&record.commit_id),
                    record.description,
                    bookmarks,
                    suffix
                )
            };
            TuiChange {
                change_id: record.change_id,
                commit_id: record.commit_id,
                label,
                parent_commit_ids: record.parent_commit_ids,
                bookmarks: record.bookmarks,
                is_workspace,
            }
        })
        .collect())
}

pub fn fail_tui_parse<T>(context: &str, result: &ParseResult<T>) -> Result<(), String> {
    parse_diagnostic(context, &result.errors, &result.raw_stdout)
        .map(Err)
        .unwrap_or(Ok(()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_state_suffix_is_inserted_after_operation_id() {
        assert_eq!(
            label_with_state_suffix("abc12345 undo def67890 [state ≈ xyz]", " S1 (= oldop)"),
            "abc12345 S1 (= oldop) undo def67890 [state ≈ xyz]"
        );
    }
}
