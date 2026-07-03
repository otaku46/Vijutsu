use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::core::OperationGraph;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WatchEvent {
    pub event_type: String,
    pub source: String,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct WatcherState {
    pub active_watcher: String,
    pub last_known_heads: Vec<String>,
    pub pending_refresh: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MultipleHeadsView {
    pub head_count: usize,
    pub head_ids: Vec<String>,
    pub message: String,
    pub selected_head: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct FilesystemFingerprint {
    pub file_count: u64,
    pub newest_modified_ms: u128,
}

pub fn detect_operation_head_change(
    state: &WatcherState,
    current_heads: Vec<String>,
) -> Option<WatchEvent> {
    (canonical_heads(state.last_known_heads.clone()) != canonical_heads(current_heads)).then(|| {
        WatchEvent {
            event_type: "op_head_change".to_string(),
            source: if state.active_watcher.is_empty() {
                "polling".to_string()
            } else {
                state.active_watcher.clone()
            },
            paths: Vec::new(),
        }
    })
}

pub fn detect_multiple_heads(graph: &OperationGraph) -> MultipleHeadsView {
    let head_ids = canonical_heads(graph.head_ids.clone());
    MultipleHeadsView {
        head_count: head_ids.len(),
        selected_head: (head_ids.len() == 1).then(|| head_ids[0].clone()),
        message: if head_ids.len() > 1 {
            "No operation was integrated. Select a head to inspect.".to_string()
        } else {
            "Single operation head.".to_string()
        },
        head_ids,
    }
}

pub fn update_watcher_state(state: &mut WatcherState, current_heads: Vec<String>) {
    state.last_known_heads = canonical_heads(current_heads);
    state.pending_refresh = false;
    if state.active_watcher.is_empty() {
        state.active_watcher = "polling".to_string();
    }
}

pub fn scan_filesystem(root: impl AsRef<Path>) -> std::io::Result<FilesystemFingerprint> {
    let mut fingerprint = FilesystemFingerprint::default();
    scan_dir(root.as_ref(), &mut fingerprint)?;
    Ok(fingerprint)
}

pub fn detect_filesystem_change(
    before: FilesystemFingerprint,
    after: FilesystemFingerprint,
) -> Option<WatchEvent> {
    (before != after).then(|| WatchEvent {
        event_type: "fs_change".to_string(),
        source: "fs_scan".to_string(),
        paths: Vec::new(),
    })
}

fn scan_dir(path: &Path, fingerprint: &mut FilesystemFingerprint) -> std::io::Result<()> {
    for entry in fs::read_dir(path)? {
        let entry = entry?;
        let path = entry.path();
        if should_skip(&path) {
            continue;
        }

        let metadata = entry.metadata()?;
        if metadata.is_dir() {
            scan_dir(&path, fingerprint)?;
        } else if metadata.is_file() {
            fingerprint.file_count += 1;
            let modified_ms = metadata
                .modified()
                .ok()
                .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
                .map(|duration| duration.as_millis())
                .unwrap_or(0);
            fingerprint.newest_modified_ms = fingerprint.newest_modified_ms.max(modified_ms);
        }
    }
    Ok(())
}

fn should_skip(path: &Path) -> bool {
    path.file_name()
        .and_then(|name| name.to_str())
        .map(|name| matches!(name, ".git" | ".jj" | "target"))
        .unwrap_or(false)
}

fn canonical_heads(mut heads: Vec<String>) -> Vec<String> {
    heads.sort();
    heads.dedup();
    heads
}

#[cfg(test)]
mod tests {
    use crate::core::OperationGraph;

    use super::*;

    #[test]
    fn detects_head_change_independent_of_order() {
        let state = WatcherState {
            active_watcher: "polling".to_string(),
            last_known_heads: vec!["b".to_string(), "a".to_string()],
            pending_refresh: false,
        };

        assert!(
            detect_operation_head_change(&state, vec!["a".to_string(), "b".to_string()]).is_none()
        );
        assert!(detect_operation_head_change(&state, vec!["c".to_string()]).is_some());
    }

    #[test]
    fn multiple_heads_view_never_merges_heads() {
        let view = detect_multiple_heads(&OperationGraph {
            nodes: Vec::new(),
            head_ids: vec!["op-b".to_string(), "op-a".to_string()],
        });

        assert_eq!(view.head_count, 2);
        assert_eq!(view.head_ids, vec!["op-a", "op-b"]);
        assert!(view.selected_head.is_none());
        assert!(view.message.contains("No operation was integrated"));
    }

    #[test]
    fn filesystem_change_detects_fingerprint_difference() {
        let before = FilesystemFingerprint {
            file_count: 1,
            newest_modified_ms: 10,
        };
        let after = FilesystemFingerprint {
            file_count: 2,
            newest_modified_ms: 10,
        };

        let event = detect_filesystem_change(before, after).unwrap();

        assert_eq!(event.event_type, "fs_change");
        assert_eq!(event.source, "fs_scan");
        assert!(detect_filesystem_change(before, before).is_none());
    }
}
