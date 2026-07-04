use std::collections::HashMap;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

use crate::jj::ChangeRecord;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct StateFingerprint {
    pub change_graph: u64,
    pub bookmarks: u64,
    pub workspaces: u64,
}

pub fn state_suffixes(op_ids: &[String], fingerprints: &[Option<StateFingerprint>]) -> Vec<String> {
    let mut state_numbers = HashMap::<StateFingerprint, usize>::new();
    let mut oldest_visible = HashMap::<StateFingerprint, &str>::new();

    for (op_id, fingerprint) in op_ids.iter().zip(fingerprints) {
        let Some(fingerprint) = fingerprint else {
            continue;
        };
        oldest_visible.insert(*fingerprint, op_id.as_str());
    }

    fingerprints
        .iter()
        .enumerate()
        .map(|(index, fingerprint)| {
            let Some(fingerprint) = fingerprint else {
                return " S?".to_string();
            };
            let next = state_numbers.len() + 1;
            let number = *state_numbers.entry(*fingerprint).or_insert(next);
            let same_as = oldest_visible
                .get(fingerprint)
                .filter(|oldest| **oldest != op_ids[index])
                .map(|oldest| format!(" (= {})", short(oldest)))
                .unwrap_or_default();
            format!(" S{number}{same_as}")
        })
        .collect()
}

pub fn state_fingerprint(records: &[ChangeRecord]) -> StateFingerprint {
    let mut changes = Vec::new();
    let mut bookmarks = Vec::new();
    let mut workspaces = Vec::new();

    for record in records {
        let mut parents = record.parent_commit_ids.clone();
        parents.sort();
        changes.push(format!("{}<-{}", record.commit_id, parents.join(",")));

        for bookmark in &record.bookmarks {
            bookmarks.push(format!("{bookmark}->{}", record.commit_id));
        }
        for workspace in &record.working_copies {
            workspaces.push(format!("{workspace}->{}", record.commit_id));
        }
    }

    changes.sort();
    bookmarks.sort();
    workspaces.sort();

    StateFingerprint {
        change_graph: hash_lines(&changes),
        bookmarks: hash_lines(&bookmarks),
        workspaces: hash_lines(&workspaces),
    }
}

fn hash_lines(lines: &[String]) -> u64 {
    let mut hasher = DefaultHasher::new();
    lines.hash(&mut hasher);
    hasher.finish()
}

fn short(value: &str) -> &str {
    value.get(..8).unwrap_or(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_suffixes_reuse_state_number_and_point_to_oldest_visible_op() {
        let same = StateFingerprint {
            change_graph: 1,
            bookmarks: 2,
            workspaces: 3,
        };
        let other = StateFingerprint {
            change_graph: 4,
            bookmarks: 2,
            workspaces: 3,
        };
        let op_ids = vec![
            "op-new".to_string(),
            "op-mid".to_string(),
            "op-old".to_string(),
        ];

        assert_eq!(
            state_suffixes(&op_ids, &[Some(same), Some(other), Some(same)]),
            vec![" S1 (= op-old)", " S2", " S1"]
        );
    }

    #[test]
    fn state_fingerprint_tracks_bookmark_plane_separately() {
        let without_bookmark = state_fingerprint(&[change_record("commit", &[], &[], &[])]);
        let with_bookmark = state_fingerprint(&[change_record("commit", &[], &["feature"], &[])]);

        assert_eq!(without_bookmark.change_graph, with_bookmark.change_graph);
        assert_ne!(without_bookmark.bookmarks, with_bookmark.bookmarks);
        assert_eq!(without_bookmark.workspaces, with_bookmark.workspaces);
    }

    fn change_record(
        commit_id: &str,
        parents: &[&str],
        bookmarks: &[&str],
        working_copies: &[&str],
    ) -> ChangeRecord {
        ChangeRecord {
            schema: 1,
            kind: "change".to_string(),
            source_jj_version: "test".to_string(),
            change_id: format!("{commit_id}-change"),
            commit_id: commit_id.to_string(),
            parent_commit_ids: parents.iter().map(|parent| parent.to_string()).collect(),
            predecessor_commit_ids: Vec::new(),
            description: String::new(),
            bookmarks: bookmarks
                .iter()
                .map(|bookmark| bookmark.to_string())
                .collect(),
            working_copies: working_copies
                .iter()
                .map(|workspace| workspace.to_string())
                .collect(),
            is_empty: false,
            has_conflict: false,
            is_immutable: false,
            is_hidden: false,
            is_divergent: false,
        }
    }
}
