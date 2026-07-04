use std::collections::{HashMap, HashSet};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::cli::{ChangeRecord, OperationRecord, ParseResult};
use crate::core::{
    ChangeGraph, ChangeNode, ConflictTransition, ObservationFreshness, ObservationToken,
    OperationDelta, OperationGraph, ParentChange, RepositoryObservation, RewritePair,
};

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SelectionState {
    pub selected_op_id: Option<String>,
    pub selected_change_id: Option<String>,
    pub selected_commit_id: Option<String>,
    pub comparison_op_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HeadValidation {
    pub heads_before: Vec<String>,
    pub heads_after: Vec<String>,
    pub is_consistent: bool,
}

pub fn change_graph_from_parse(parse: ParseResult<ChangeRecord>) -> ChangeGraph {
    let nodes: Vec<ChangeNode> = parse
        .records
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    let root_ids = nodes
        .iter()
        .filter(|node| node.parent_commit_ids.is_empty())
        .map(|node| node.commit_id.clone())
        .collect();

    ChangeGraph { nodes, root_ids }
}

pub fn operation_graph_from_parse(parse: ParseResult<OperationRecord>) -> OperationGraph {
    let mut nodes = parse
        .records
        .into_iter()
        .map(Into::into)
        .collect::<Vec<_>>();
    let parent_ids = nodes
        .iter()
        .flat_map(|node: &crate::core::OperationNode| node.parent_operation_ids.iter().cloned())
        .collect::<HashSet<_>>();
    let head_ids = nodes
        .iter()
        .filter(|node| !parent_ids.contains(&node.operation_id))
        .map(|node| node.operation_id.clone())
        .collect::<Vec<_>>();

    for node in &mut nodes {
        node.is_head = head_ids.contains(&node.operation_id);
    }

    OperationGraph { nodes, head_ids }
}

pub fn compute_operation_delta(
    from_op: impl Into<String>,
    to_op: impl Into<String>,
    old_graph: &ChangeGraph,
    new_graph: &ChangeGraph,
) -> OperationDelta {
    let old_by_commit = old_graph
        .nodes
        .iter()
        .map(|node| (node.commit_id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let new_by_commit = new_graph
        .nodes
        .iter()
        .map(|node| (node.commit_id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let old_by_change = old_graph
        .nodes
        .iter()
        .map(|node| (node.change_id.as_str(), node))
        .collect::<HashMap<_, _>>();
    let new_by_change = new_graph
        .nodes
        .iter()
        .map(|node| (node.change_id.as_str(), node))
        .collect::<HashMap<_, _>>();

    let added_commits = new_by_commit
        .keys()
        .filter(|commit_id| !old_by_commit.contains_key(**commit_id))
        .map(|commit_id| (*commit_id).to_string())
        .collect();
    let removed_commits = old_by_commit
        .keys()
        .filter(|commit_id| !new_by_commit.contains_key(**commit_id))
        .map(|commit_id| (*commit_id).to_string())
        .collect();

    let rewritten_changes = old_by_change
        .iter()
        .filter_map(|(change_id, old)| {
            let new = new_by_change.get(change_id)?;
            (old.commit_id != new.commit_id).then(|| RewritePair {
                change_id: (*change_id).to_string(),
                old_commit_id: old.commit_id.clone(),
                new_commit_id: new.commit_id.clone(),
            })
        })
        .collect();

    let parent_changes = old_by_commit
        .iter()
        .filter_map(|(commit_id, old)| {
            let new = new_by_commit.get(commit_id)?;
            (old.parent_commit_ids != new.parent_commit_ids).then(|| ParentChange {
                commit_id: (*commit_id).to_string(),
                old_parents: old.parent_commit_ids.clone(),
                new_parents: new.parent_commit_ids.clone(),
            })
        })
        .collect();

    let conflict_transitions = old_by_commit
        .iter()
        .filter_map(|(commit_id, old)| {
            let new = new_by_commit.get(commit_id)?;
            (old.flags.has_conflict != new.flags.has_conflict).then(|| ConflictTransition {
                commit_id: (*commit_id).to_string(),
                had_conflict: old.flags.has_conflict,
                has_conflict: new.flags.has_conflict,
            })
        })
        .collect();

    OperationDelta {
        from_op: from_op.into(),
        to_op: to_op.into(),
        added_commits,
        removed_commits,
        rewritten_changes,
        parent_changes,
        moved_bookmarks: Vec::new(),
        workspace_moves: Vec::new(),
        conflict_transitions,
    }
}

pub fn build_observation(
    selected_operation_id: String,
    operation_graph: OperationGraph,
    change_graph: ChangeGraph,
    freshness: ObservationFreshness,
) -> RepositoryObservation {
    let head_set_hash = operation_graph.head_ids.join(",");
    let observed_at_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_millis())
        .unwrap_or_default();

    RepositoryObservation {
        selected_operation_id: selected_operation_id.clone(),
        operation_heads: operation_graph.head_ids.clone(),
        operation_graph,
        change_graph,
        freshness,
        token: ObservationToken {
            operation_id: selected_operation_id,
            head_set_hash,
            observed_at_ms,
        },
    }
}

pub fn validate_head_consistency(
    heads_before: Vec<String>,
    heads_after: Vec<String>,
) -> HeadValidation {
    let before = canonical_heads(heads_before);
    let after = canonical_heads(heads_after);

    HeadValidation {
        is_consistent: before == after,
        heads_before: before,
        heads_after: after,
    }
}

pub fn resolve_operation_selection(
    previous: &SelectionState,
    graph: &OperationGraph,
) -> SelectionState {
    let selected_op_id = previous
        .selected_op_id
        .as_deref()
        .and_then(|id| find_operation_exact(graph, id))
        .or_else(|| {
            previous
                .selected_op_id
                .as_deref()
                .and_then(|id| find_operation_child(graph, id))
        })
        .or_else(|| {
            previous
                .selected_op_id
                .as_deref()
                .and_then(|id| find_operation_parent(graph, id))
        })
        .or_else(|| {
            graph
                .nodes
                .iter()
                .find(|node| node.is_selected)
                .map(|node| node.operation_id.clone())
        })
        .or_else(|| graph.head_ids.first().cloned())
        .or_else(|| graph.nodes.first().map(|node| node.operation_id.clone()));

    SelectionState {
        selected_op_id,
        selected_change_id: previous.selected_change_id.clone(),
        selected_commit_id: previous.selected_commit_id.clone(),
        comparison_op_id: previous.comparison_op_id.clone(),
    }
}

pub fn resolve_change_selection(previous: &SelectionState, graph: &ChangeGraph) -> SelectionState {
    let selected = find_same_change_and_commit(previous, graph)
        .or_else(|| find_change_by_id(previous.selected_change_id.as_deref(), graph))
        .or_else(|| find_change_by_commit(previous.selected_commit_id.as_deref(), graph))
        .or_else(|| find_rewrite_neighbor(previous.selected_commit_id.as_deref(), graph))
        .or_else(|| find_parent_change(previous.selected_commit_id.as_deref(), graph))
        .or_else(|| find_working_copy_change(graph))
        .or_else(|| {
            graph
                .nodes
                .iter()
                .find(|node| node.commit_id != "0000000000000000000000000000000000000000")
        })
        .or_else(|| graph.nodes.first());

    SelectionState {
        selected_op_id: previous.selected_op_id.clone(),
        selected_change_id: selected.map(|node| node.change_id.clone()),
        selected_commit_id: selected.map(|node| node.commit_id.clone()),
        comparison_op_id: previous.comparison_op_id.clone(),
    }
}

fn canonical_heads(mut heads: Vec<String>) -> Vec<String> {
    heads.sort();
    heads.dedup();
    heads
}

fn find_operation_exact(graph: &OperationGraph, id: &str) -> Option<String> {
    graph
        .nodes
        .iter()
        .find(|node| node.operation_id == id)
        .map(|node| node.operation_id.clone())
}

fn find_operation_child(graph: &OperationGraph, id: &str) -> Option<String> {
    graph
        .nodes
        .iter()
        .find(|node| node.parent_operation_ids.iter().any(|parent| parent == id))
        .map(|node| node.operation_id.clone())
}

fn find_operation_parent(graph: &OperationGraph, id: &str) -> Option<String> {
    let node = graph.nodes.iter().find(|node| node.operation_id == id)?;
    node.parent_operation_ids
        .iter()
        .find_map(|parent_id| find_operation_exact(graph, parent_id))
}

fn find_same_change_and_commit<'a>(
    selection: &SelectionState,
    graph: &'a ChangeGraph,
) -> Option<&'a ChangeNode> {
    let change_id = selection.selected_change_id.as_deref()?;
    let commit_id = selection.selected_commit_id.as_deref()?;
    graph
        .nodes
        .iter()
        .find(|node| node.change_id == change_id && node.commit_id == commit_id)
}

fn find_change_by_id<'a>(
    change_id: Option<&str>,
    graph: &'a ChangeGraph,
) -> Option<&'a ChangeNode> {
    let change_id = change_id?;
    let mut matches = graph
        .nodes
        .iter()
        .filter(|node| id_matches(&node.change_id, change_id))
        .collect::<Vec<_>>();
    (matches.len() == 1).then(|| matches.remove(0))
}

fn find_change_by_commit<'a>(
    commit_id: Option<&str>,
    graph: &'a ChangeGraph,
) -> Option<&'a ChangeNode> {
    let commit_id = commit_id?;
    graph
        .nodes
        .iter()
        .find(|node| id_matches(&node.commit_id, commit_id))
}

fn find_rewrite_neighbor<'a>(
    commit_id: Option<&str>,
    graph: &'a ChangeGraph,
) -> Option<&'a ChangeNode> {
    let commit_id = commit_id?;
    graph.nodes.iter().find(|node| {
        node.predecessor_commit_ids
            .iter()
            .any(|predecessor| predecessor == commit_id)
    })
}

fn find_parent_change<'a>(
    commit_id: Option<&str>,
    graph: &'a ChangeGraph,
) -> Option<&'a ChangeNode> {
    let commit_id = commit_id?;
    graph.nodes.iter().find(|node| {
        node.parent_commit_ids
            .iter()
            .any(|parent| parent == commit_id)
    })
}

fn find_working_copy_change(graph: &ChangeGraph) -> Option<&ChangeNode> {
    graph
        .nodes
        .iter()
        .find(|node| !node.workspace_names.is_empty())
}

fn id_matches(full: &str, query: &str) -> bool {
    full == query || full.starts_with(query)
}

#[cfg(test)]
mod tests {
    use crate::cli::parse_jsonl;
    use crate::core::{ChangeFlags, ChangeNode};

    use super::*;

    fn node(change_id: &str, commit_id: &str, parents: &[&str], conflict: bool) -> ChangeNode {
        ChangeNode {
            change_id: change_id.to_string(),
            commit_id: commit_id.to_string(),
            parent_commit_ids: parents.iter().map(|item| item.to_string()).collect(),
            predecessor_commit_ids: Vec::new(),
            description: String::new(),
            bookmarks: Vec::new(),
            workspace_names: Vec::new(),
            flags: ChangeFlags {
                has_conflict: conflict,
                ..ChangeFlags::default()
            },
        }
    }

    #[test]
    fn delta_detects_added_removed_rewrite_parent_and_conflict_changes() {
        let old_graph = ChangeGraph {
            nodes: vec![
                node("c1", "k1", &[], false),
                node("c2", "k2", &["k1"], false),
            ],
            root_ids: vec!["k1".to_string()],
        };
        let new_graph = ChangeGraph {
            nodes: vec![
                node("c1", "k1", &["base"], true),
                node("c2", "k3", &["k1"], false),
            ],
            root_ids: vec!["k1".to_string()],
        };

        let delta = compute_operation_delta("o1", "o2", &old_graph, &new_graph);

        assert_eq!(delta.added_commits, vec!["k3"]);
        assert_eq!(delta.removed_commits, vec!["k2"]);
        assert_eq!(delta.rewritten_changes[0].change_id, "c2");
        assert_eq!(delta.parent_changes[0].commit_id, "k1");
        assert_eq!(delta.conflict_transitions[0].commit_id, "k1");
    }

    #[test]
    fn head_validation_ignores_order_but_detects_changes() {
        let same = validate_head_consistency(
            vec!["b".to_string(), "a".to_string()],
            vec!["a".to_string(), "b".to_string()],
        );
        let changed = validate_head_consistency(vec!["a".to_string()], vec!["b".to_string()]);

        assert!(same.is_consistent);
        assert!(!changed.is_consistent);
    }

    #[test]
    fn operation_selection_restores_same_child_parent_then_head() {
        let graph = OperationGraph {
            nodes: vec![
                crate::core::OperationNode {
                    operation_id: "parent".to_string(),
                    parent_operation_ids: Vec::new(),
                    description: String::new(),
                    timestamp_start: None,
                    timestamp_end: None,
                    user: None,
                    host: None,
                    is_head: false,
                    is_selected: false,
                },
                crate::core::OperationNode {
                    operation_id: "child".to_string(),
                    parent_operation_ids: vec!["parent".to_string()],
                    description: String::new(),
                    timestamp_start: None,
                    timestamp_end: None,
                    user: None,
                    host: None,
                    is_head: true,
                    is_selected: false,
                },
            ],
            head_ids: vec!["child".to_string()],
        };

        let same = resolve_operation_selection(
            &SelectionState {
                selected_op_id: Some("child".to_string()),
                ..SelectionState::default()
            },
            &graph,
        );
        let child = resolve_operation_selection(
            &SelectionState {
                selected_op_id: Some("parent".to_string()),
                ..SelectionState::default()
            },
            &graph,
        );
        let head = resolve_operation_selection(&SelectionState::default(), &graph);

        assert_eq!(same.selected_op_id.as_deref(), Some("child"));
        assert_eq!(child.selected_op_id.as_deref(), Some("parent"));
        assert_eq!(head.selected_op_id.as_deref(), Some("child"));
    }

    #[test]
    fn change_selection_restores_by_prefix_and_avoids_ambiguous_change_ids() {
        let graph = ChangeGraph {
            nodes: vec![
                node("changeaaa", "commitaaa", &[], false),
                node("changebbb", "commitbbb", &["commitaaa"], false),
            ],
            root_ids: vec!["commitaaa".to_string()],
        };

        let by_change_prefix = resolve_change_selection(
            &SelectionState {
                selected_change_id: Some("changeb".to_string()),
                ..SelectionState::default()
            },
            &graph,
        );
        let by_commit_prefix = resolve_change_selection(
            &SelectionState {
                selected_commit_id: Some("commita".to_string()),
                ..SelectionState::default()
            },
            &graph,
        );

        assert_eq!(
            by_change_prefix.selected_commit_id.as_deref(),
            Some("commitbbb")
        );
        assert_eq!(
            by_commit_prefix.selected_change_id.as_deref(),
            Some("changeaaa")
        );
    }

    #[test]
    fn change_graph_roots_are_parentless_nodes() {
        let graph = ChangeGraph {
            nodes: vec![
                node("root", "k0", &[], false),
                node("child", "k1", &["k0"], false),
            ],
            root_ids: Vec::new(),
        };
        let records = graph
            .nodes
            .into_iter()
            .map(|node| ChangeRecord {
                schema: 1,
                kind: "change".to_string(),
                source_jj_version: "jj test".to_string(),
                change_id: node.change_id,
                commit_id: node.commit_id,
                parent_commit_ids: node.parent_commit_ids,
                predecessor_commit_ids: Vec::new(),
                description: String::new(),
                bookmarks: Vec::new(),
                working_copies: Vec::new(),
                is_empty: false,
                has_conflict: false,
                is_immutable: false,
                is_hidden: false,
                is_divergent: false,
            })
            .collect();

        let parsed = change_graph_from_parse(ParseResult {
            records,
            errors: Vec::new(),
            raw_stdout: String::new(),
        });

        assert_eq!(parsed.root_ids, vec!["k0"]);
    }

    #[test]
    fn golden_change_log_converts_to_domain_graph() {
        let graph = change_graph_from_parse(parse_jsonl(
            include_str!("../tests/fixtures/jj/change_log.jsonl"),
            "change",
        ));

        assert_eq!(
            graph.root_ids,
            vec!["0000000000000000000000000000000000000000"]
        );
        let base = graph
            .nodes
            .iter()
            .find(|node| node.commit_id == "commit-a")
            .unwrap();
        assert_eq!(base.bookmarks[0].name, "main");
        assert_eq!(base.bookmarks[0].remote, None);
        assert_eq!(base.bookmarks[1].name, "main");
        assert_eq!(base.bookmarks[1].remote.as_deref(), Some("origin"));

        let working = graph
            .nodes
            .iter()
            .find(|node| node.commit_id == "commit-b")
            .unwrap();
        assert_eq!(working.workspace_names, vec!["default"]);
        assert!(working.flags.has_conflict);
        assert!(working.flags.is_divergent);
    }

    #[test]
    fn golden_operation_log_converts_to_domain_graph() {
        let graph = operation_graph_from_parse(parse_jsonl(
            include_str!("../tests/fixtures/jj/op_log.jsonl"),
            "operation",
        ));

        assert_eq!(graph.head_ids, vec!["op-new"]);
        let current = graph
            .nodes
            .iter()
            .find(|node| node.operation_id == "op-new")
            .unwrap();
        assert!(current.is_head);
        assert!(current.is_selected);
        assert_eq!(current.parent_operation_ids, vec!["op-mid"]);
    }
}
