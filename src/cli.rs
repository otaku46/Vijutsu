use serde::Deserialize;
use serde::de::DeserializeOwned;
use std::ffi::OsString;
use std::process::Command;
use std::time::Instant;

use crate::core::{Bookmark, ChangeFlags, ChangeNode, JjCapabilities, OperationNode};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandEnvelope {
    pub binary_path: String,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommandResult {
    pub exit_code: i32,
    pub stdout: String,
    pub stderr: String,
    pub elapsed_ms: u128,
    pub command_display: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseError {
    pub line_number: usize,
    pub raw_line: String,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParseResult<T> {
    pub records: Vec<T>,
    pub errors: Vec<ParseError>,
    pub raw_stdout: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileEntry {
    pub path: String,
    pub status: String,
}

pub trait ProtocolRecord {
    fn schema_version(&self) -> u32;
    fn kind(&self) -> &str;
    fn source_jj_version(&self) -> &str;
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct ChangeRecord {
    pub schema: u32,
    pub kind: String,
    pub source_jj_version: String,
    pub change_id: String,
    pub commit_id: String,
    #[serde(default)]
    pub parent_commit_ids: Vec<String>,
    #[serde(default)]
    pub predecessor_commit_ids: Vec<String>,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub bookmarks: Vec<String>,
    #[serde(default)]
    pub working_copies: Vec<String>,
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

impl ProtocolRecord for ChangeRecord {
    fn schema_version(&self) -> u32 {
        self.schema
    }

    fn kind(&self) -> &str {
        &self.kind
    }

    fn source_jj_version(&self) -> &str {
        &self.source_jj_version
    }
}

impl From<ChangeRecord> for ChangeNode {
    fn from(record: ChangeRecord) -> Self {
        Self {
            change_id: record.change_id,
            commit_id: record.commit_id,
            parent_commit_ids: record.parent_commit_ids,
            predecessor_commit_ids: record.predecessor_commit_ids,
            description: record.description,
            bookmarks: record
                .bookmarks
                .into_iter()
                .map(bookmark_from_template_name)
                .collect(),
            workspace_names: record.working_copies,
            flags: ChangeFlags {
                is_empty: record.is_empty,
                has_conflict: record.has_conflict,
                is_immutable: record.is_immutable,
                is_hidden: record.is_hidden,
                is_divergent: record.is_divergent,
            },
        }
    }
}

fn bookmark_from_template_name(raw: String) -> Bookmark {
    let Some((name, remote)) = raw.rsplit_once('@') else {
        return Bookmark {
            name: raw,
            remote: None,
            is_conflicted: false,
        };
    };
    Bookmark {
        name: name.to_string(),
        remote: Some(remote.to_string()),
        is_conflicted: false,
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct OperationRecord {
    pub schema: u32,
    pub kind: String,
    pub source_jj_version: String,
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
    pub is_current: bool,
}

impl ProtocolRecord for OperationRecord {
    fn schema_version(&self) -> u32 {
        self.schema
    }

    fn kind(&self) -> &str {
        &self.kind
    }

    fn source_jj_version(&self) -> &str {
        &self.source_jj_version
    }
}

impl From<OperationRecord> for OperationNode {
    fn from(record: OperationRecord) -> Self {
        Self {
            operation_id: record.operation_id,
            parent_operation_ids: record.parent_operation_ids,
            description: record.description,
            timestamp_start: record.timestamp_start,
            timestamp_end: record.timestamp_end,
            user: record.user,
            host: record.host,
            is_head: false,
            is_selected: record.is_current,
        }
    }
}

pub fn isolated_jj(args: impl IntoIterator<Item = impl Into<String>>) -> CommandEnvelope {
    let mut final_args = vec![
        "--color=never".to_string(),
        "--no-pager".to_string(),
        "--quiet".to_string(),
    ];
    final_args.extend(args.into_iter().map(Into::into));

    CommandEnvelope {
        binary_path: "jj".to_string(),
        args: final_args,
        env: vec![("JJ_CONFIG".to_string(), String::new())],
    }
}

pub fn execute(envelope: &CommandEnvelope) -> std::io::Result<CommandResult> {
    let started = Instant::now();
    let output = Command::new(&envelope.binary_path)
        .args(&envelope.args)
        .envs(
            envelope
                .env
                .iter()
                .map(|(k, v)| (OsString::from(k), OsString::from(v))),
        )
        .output()?;

    Ok(CommandResult {
        exit_code: output.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&output.stderr).into_owned(),
        elapsed_ms: started.elapsed().as_millis(),
        command_display: format!("{} {}", envelope.binary_path, envelope.args.join(" ")),
    })
}

pub fn parse_jsonl<T>(stdout: &str, expected_kind: &str) -> ParseResult<T>
where
    T: DeserializeOwned + ProtocolRecord,
{
    let mut records = Vec::new();
    let mut errors = Vec::new();

    for (index, line) in stdout.lines().enumerate() {
        let line_number = index + 1;
        if line.trim().is_empty() {
            continue;
        }

        match serde_json::from_str::<T>(line) {
            Ok(record) if record.schema_version() != 1 => errors.push(ParseError {
                line_number,
                raw_line: line.to_string(),
                message: format!("unsupported schema version {}", record.schema_version()),
            }),
            Ok(record) if record.kind() != expected_kind => errors.push(ParseError {
                line_number,
                raw_line: line.to_string(),
                message: format!("expected kind {expected_kind}, got {}", record.kind()),
            }),
            Ok(record) if record.source_jj_version().is_empty() => errors.push(ParseError {
                line_number,
                raw_line: line.to_string(),
                message: "missing source_jj_version".to_string(),
            }),
            Ok(record) => records.push(record),
            Err(error) => errors.push(ParseError {
                line_number,
                raw_line: line.to_string(),
                message: error.to_string(),
            }),
        }
    }

    ParseResult {
        records,
        errors,
        raw_stdout: stdout.to_string(),
    }
}

pub fn jj_version() -> std::io::Result<CommandResult> {
    execute(&isolated_jj(["version"]))
}

pub fn detect_capabilities() -> std::io::Result<JjCapabilities> {
    let version_result = jj_version()?;
    ensure_success(&version_result)?;
    let version = version_result.stdout.trim().to_string();

    let log_help = probe_stdout(["help", "log"])?;
    let restore_help = probe_stdout(["help", "op", "restore"])?;

    Ok(JjCapabilities {
        version,
        json_template_function: probe_success([
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r",
            "root()",
            "-T",
            "json(commit_id) ++ \"\\n\"",
        ]),
        escape_json_method: probe_success([
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r",
            "root()",
            "-T",
            "description.escape_json() ++ \"\\n\"",
        ]),
        operation_parent_template: probe_success([
            "--ignore-working-copy",
            "op",
            "log",
            "--no-graph",
            "--limit",
            "1",
            "-T",
            "json(parents.map(|o| o.id())) ++ \"\\n\"",
        ]),
        // The current MVP template does not consume commit evolution data yet.
        commit_predecessor_template: false,
        op_diff: probe_success(["help", "op", "diff"]),
        op_show: probe_success(["help", "op", "show"]),
        op_revert: probe_success(["help", "op", "revert"]),
        op_restore: probe_success(["help", "op", "restore"]),
        op_restore_what: restore_help.contains("--what"),
        redo: probe_success(["help", "redo"]),
        util_snapshot: probe_success(["help", "util", "snapshot"]),
        no_integrate_operation: log_help.contains("--no-integrate-operation"),
    })
}

fn probe_success(args: impl IntoIterator<Item = impl Into<String>>) -> bool {
    execute(&isolated_jj(args))
        .map(|result| result.exit_code == 0)
        .unwrap_or(false)
}

fn probe_stdout(args: impl IntoIterator<Item = impl Into<String>>) -> std::io::Result<String> {
    let result = execute(&isolated_jj(args))?;
    ensure_success(&result)?;
    Ok(result.stdout)
}

pub fn fetch_change_records(
    source_jj_version: &str,
    at_op: &str,
    revset: &str,
) -> std::io::Result<ParseResult<ChangeRecord>> {
    let result = execute(&isolated_jj([
        "--ignore-working-copy",
        "--at-op",
        at_op,
        "log",
        "--no-graph",
        "-r",
        revset,
        "-T",
        &change_jsonl_template(source_jj_version),
    ]))?;
    ensure_success(&result)?;

    Ok(parse_jsonl::<ChangeRecord>(&result.stdout, "change"))
}

pub fn fetch_operation_records(
    source_jj_version: &str,
    at_op: &str,
    limit: usize,
) -> std::io::Result<ParseResult<OperationRecord>> {
    let result = execute(&isolated_jj([
        "--ignore-working-copy",
        "--at-op",
        at_op,
        "op",
        "log",
        "--no-graph",
        "--limit",
        &limit.to_string(),
        "-T",
        &operation_jsonl_template(source_jj_version),
    ]))?;
    ensure_success(&result)?;

    Ok(parse_jsonl::<OperationRecord>(&result.stdout, "operation"))
}

pub fn fetch_file_summary(at_op: &str, revision: &str) -> std::io::Result<Vec<FileEntry>> {
    let result = execute(&isolated_jj([
        "--ignore-working-copy",
        "--at-op",
        at_op,
        "diff",
        "-r",
        revision,
        "--summary",
    ]))?;
    ensure_success(&result)?;

    Ok(parse_diff_summary(&result.stdout))
}

pub fn fetch_diff(at_op: &str, revision: &str) -> std::io::Result<String> {
    let result = execute(&isolated_jj([
        "--ignore-working-copy",
        "--at-op",
        at_op,
        "diff",
        "-r",
        revision,
        "--git",
    ]))?;
    ensure_success(&result)?;

    Ok(result.stdout)
}

pub fn fetch_diff_for_path(at_op: &str, revision: &str, path: &str) -> std::io::Result<String> {
    let result = execute(&isolated_jj([
        "--ignore-working-copy",
        "--at-op",
        at_op,
        "diff",
        "-r",
        revision,
        "--git",
        "--",
        path,
    ]))?;
    ensure_success(&result)?;

    Ok(result.stdout)
}

pub fn fetch_bookmark_list() -> std::io::Result<CommandResult> {
    execute(&isolated_jj(["--ignore-working-copy", "bookmark", "list"]))
}

pub fn execute_snapshot_refresh() -> std::io::Result<CommandResult> {
    let result = execute(&isolated_jj(["util", "snapshot"]))?;
    ensure_success(&result)?;
    Ok(result)
}

pub fn parse_diff_summary(stdout: &str) -> Vec<FileEntry> {
    stdout
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            if trimmed.is_empty() {
                return None;
            }
            let (status, path) = trimmed.split_once(' ')?;
            Some(FileEntry {
                status: status.to_string(),
                path: path.trim().to_string(),
            })
        })
        .collect()
}

fn ensure_success(result: &CommandResult) -> std::io::Result<()> {
    if result.exit_code == 0 {
        return Ok(());
    }

    Err(std::io::Error::other(format!(
        "{} failed with exit code {}\n{}",
        result.command_display,
        result.exit_code,
        result.stderr.trim()
    )))
}

fn change_jsonl_template(source_jj_version: &str) -> String {
    let version = jj_string_literal_json(source_jj_version);
    format!(
        r#""{{\"schema\":1,\"kind\":\"change\",\"source_jj_version\":{version},\"change_id\":" ++ json(change_id) ++ ",\"commit_id\":" ++ json(commit_id) ++ ",\"parent_commit_ids\":" ++ json(parents.map(|c| c.commit_id())) ++ ",\"predecessor_commit_ids\":[],\"description\":" ++ description.escape_json() ++ ",\"bookmarks\":" ++ json(bookmarks.map(|b| b.name())) ++ ",\"working_copies\":" ++ json(working_copies.map(|w| w.name())) ++ ",\"is_empty\":" ++ json(empty) ++ ",\"has_conflict\":" ++ json(conflict) ++ ",\"is_immutable\":" ++ json(immutable) ++ ",\"is_hidden\":" ++ json(hidden) ++ ",\"is_divergent\":" ++ json(divergent) ++ "}}\n""#
    )
}

fn operation_jsonl_template(source_jj_version: &str) -> String {
    let version = jj_string_literal_json(source_jj_version);
    format!(
        r#""{{\"schema\":1,\"kind\":\"operation\",\"source_jj_version\":{version},\"operation_id\":" ++ json(id) ++ ",\"parent_operation_ids\":" ++ json(parents.map(|o| o.id())) ++ ",\"description\":" ++ description.escape_json() ++ ",\"timestamp_start\":" ++ json(time.start()) ++ ",\"timestamp_end\":" ++ json(time.end()) ++ ",\"user\":" ++ user.escape_json() ++ ",\"host\":null,\"is_current\":" ++ json(current_operation) ++ "}}\n""#
    )
}

fn jj_string_literal_json(value: &str) -> String {
    serde_json::to_string(value)
        .expect("string serialization cannot fail")
        .replace('\\', "\\\\")
        .replace('"', "\\\"")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_jsonl_keeps_good_records_after_bad_lines() {
        let input = concat!(
            "{\"schema\":1,\"kind\":\"change\",\"source_jj_version\":\"0.33.0\",\"change_id\":\"c1\",\"commit_id\":\"k1\"}\n",
            "not json\n",
            "{\"schema\":2,\"kind\":\"change\",\"source_jj_version\":\"0.33.0\",\"change_id\":\"c2\",\"commit_id\":\"k2\"}\n",
            "{\"schema\":1,\"kind\":\"change\",\"source_jj_version\":\"0.33.0\",\"change_id\":\"c3\",\"commit_id\":\"k3\"}\n",
        );

        let parsed = parse_jsonl::<ChangeRecord>(input, "change");

        assert_eq!(parsed.records.len(), 2);
        assert_eq!(parsed.records[0].change_id, "c1");
        assert_eq!(parsed.records[1].change_id, "c3");
        assert_eq!(parsed.errors.len(), 2);
    }

    #[test]
    fn isolated_jj_disables_user_config_and_pager() {
        let envelope = isolated_jj(["log"]);

        assert!(envelope.args.contains(&"--color=never".to_string()));
        assert!(envelope.args.contains(&"--no-pager".to_string()));
        assert!(envelope.args.contains(&"--quiet".to_string()));
        assert!(
            envelope
                .env
                .contains(&("JJ_CONFIG".to_string(), String::new()))
        );
    }

    #[test]
    fn capability_probe_defaults_unknown_predecessor_to_false() {
        let caps = JjCapabilities::default();

        assert!(!caps.commit_predecessor_template);
    }

    #[test]
    fn fetch_commands_use_explicit_read_only_operation_and_template() {
        let envelope = isolated_jj([
            "--ignore-working-copy",
            "--at-op",
            "@",
            "log",
            "--no-graph",
            "-r",
            "all()",
            "-T",
            &change_jsonl_template("jj test"),
        ]);

        assert!(envelope.args.contains(&"--ignore-working-copy".to_string()));
        assert!(envelope.args.contains(&"--at-op".to_string()));
        assert!(envelope.args.contains(&"@".to_string()));
        assert!(envelope.args.contains(&"-T".to_string()));
        assert!(envelope.args.contains(&"all()".to_string()));
    }

    #[test]
    fn parse_diff_summary_extracts_status_and_path() {
        let files = parse_diff_summary("M src/main.rs\nA README.md\n");

        assert_eq!(
            files,
            vec![
                FileEntry {
                    status: "M".to_string(),
                    path: "src/main.rs".to_string(),
                },
                FileEntry {
                    status: "A".to_string(),
                    path: "README.md".to_string(),
                },
            ]
        );
    }

    #[test]
    fn bookmark_template_name_splits_remote_suffix() {
        assert_eq!(
            bookmark_from_template_name("feature".to_string()),
            Bookmark {
                name: "feature".to_string(),
                remote: None,
                is_conflicted: false,
            }
        );
        assert_eq!(
            bookmark_from_template_name("main@origin".to_string()),
            Bookmark {
                name: "main".to_string(),
                remote: Some("origin".to_string()),
                is_conflicted: false,
            }
        );
    }
}
