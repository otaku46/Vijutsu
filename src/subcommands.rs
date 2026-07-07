use std::time::Duration;

use crate::cli::{
    ParseError, ParseResult, detect_capabilities, execute_snapshot_refresh, fetch_bookmark_list,
    fetch_change_records, fetch_diff, fetch_file_summary, fetch_operation_records, jj_version,
};
use crate::core::{JjCapabilities, ObservationFreshness};
use crate::mutation::{
    RestorePreview, RevertPreview, UndoPreview, abandon_preview, execute_mutation, redo_preview,
    restore_warnings, snapshot_refresh_result, snapshot_refresh_warnings, undo_warnings,
};
use crate::observation::{
    SelectionState, build_observation, change_graph_from_parse, compute_operation_delta,
    operation_graph_from_parse, resolve_change_selection, validate_head_consistency,
};
use crate::state::guard_execution_head;
use crate::tui::{
    ChangeDetails, DeltaDetails, preview_panel_for_abandon, preview_panel_for_redo,
    preview_panel_for_restore, preview_panel_for_revert, preview_panel_for_snapshot_refresh,
    preview_panel_for_undo, render_keys, render_multiple_heads, render_observation,
    render_preview_panel,
};
use crate::tui_support::{resolve_operation_prefix, short};
use crate::watcher::{
    WatcherState, detect_filesystem_change, detect_multiple_heads, detect_operation_head_change,
    scan_filesystem, update_watcher_state,
};

pub fn inspect(args: Vec<String>) {
    let options = match InspectOptions::parse(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let version = jj_version_string();
    let filesystem_before = scan_filesystem(".")
        .unwrap_or_else(|error| fail_io("scan filesystem before inspect", error));
    let op_parse_before = fetch_operation_records(&version, "@", options.operation_limit)
        .unwrap_or_else(|error| fail_io("fetch operation graph", error));
    fail_parse_result("operation graph", &op_parse_before);

    let current_operation_id = op_parse_before
        .records
        .iter()
        .find(|record| record.is_current)
        .or_else(|| op_parse_before.records.first())
        .map(|record| record.operation_id.clone())
        .unwrap_or_else(|| "@".to_string());
    let known_operation_ids = op_parse_before
        .records
        .iter()
        .map(|record| record.operation_id.as_str())
        .collect::<Vec<_>>();
    let selected_operation_id = match options.selected_operation.as_deref() {
        Some(operation) => resolve_operation_prefix(operation, &known_operation_ids)
            .unwrap_or_else(|error| fail_message(&error)),
        None => current_operation_id.clone(),
    };
    let operation_graph_before = operation_graph_from_parse(op_parse_before);

    let change_parse = fetch_change_records(&version, &selected_operation_id, &options.revset)
        .unwrap_or_else(|error| fail_io("fetch change graph", error));
    fail_parse_result("change graph", &change_parse);
    let change_graph = change_graph_from_parse(change_parse);
    let delta = options.compare_operation.as_deref().map(|from_op| {
        let compare_parse = fetch_change_records(&version, from_op, &options.revset)
            .unwrap_or_else(|error| fail_io("fetch comparison change graph", error));
        fail_parse_result("comparison change graph", &compare_parse);
        let compare_graph = change_graph_from_parse(compare_parse);
        DeltaDetails {
            delta: compute_operation_delta(
                from_op,
                &selected_operation_id,
                &compare_graph,
                &change_graph,
            ),
        }
    });
    let selection = resolve_change_selection(
        &SelectionState {
            selected_op_id: Some(selected_operation_id.clone()),
            selected_change_id: options.selected_change.clone(),
            selected_commit_id: options.selected_change.clone(),
            comparison_op_id: None,
        },
        &change_graph,
    );
    let details = selection
        .selected_commit_id
        .as_deref()
        .and_then(|commit_id| {
            change_graph
                .nodes
                .iter()
                .find(|change| change.commit_id == commit_id)
        })
        .filter(|change| change.commit_id != "0000000000000000000000000000000000000000")
        .map(|change| {
            let files = fetch_file_summary(&selected_operation_id, &change.commit_id)
                .unwrap_or_else(|error| fail_io("fetch file summary", error));
            let diff = fetch_diff(&selected_operation_id, &change.commit_id)
                .unwrap_or_else(|error| fail_io("fetch diff", error));
            ChangeDetails {
                selected_commit_id: change.commit_id.clone(),
                files,
                diff,
            }
        });

    let op_parse_after = fetch_operation_records(&version, "@", options.operation_limit)
        .unwrap_or_else(|error| fail_io("re-check operation graph", error));
    fail_parse_result("operation graph re-check", &op_parse_after);
    let filesystem_after = scan_filesystem(".")
        .unwrap_or_else(|error| fail_io("scan filesystem after inspect", error));
    let operation_graph_after = operation_graph_from_parse(op_parse_after);
    let head_validation = validate_head_consistency(
        operation_graph_before.head_ids.clone(),
        operation_graph_after.head_ids,
    );
    let operation_limit_reached = operation_graph_before.nodes.len() >= options.operation_limit;
    let freshness = if !head_validation.is_consistent {
        ObservationFreshness::OperationHeadChanged
    } else if selected_operation_id != current_operation_id {
        ObservationFreshness::Historical {
            operation_id: selected_operation_id.clone(),
        }
    } else if detect_filesystem_change(filesystem_before, filesystem_after).is_some() {
        ObservationFreshness::FilesystemAhead {
            changed_paths_known: false,
        }
    } else {
        ObservationFreshness::CurrentSnapshot
    };

    let observation = build_observation(
        selected_operation_id,
        operation_graph_before,
        change_graph,
        freshness,
    );
    let mut rendered = render_observation(&observation, details.as_ref(), delta.as_ref());
    if operation_limit_reached {
        rendered.push_str(&format!(
            "\nOperation limit reached: showing {} operations; older operations may exist.\n",
            options.operation_limit
        ));
    }
    print!("{rendered}");
}

pub fn print_version() {
    match jj_version() {
        Ok(result) if result.exit_code == 0 => print!("{}", result.stdout),
        Ok(result) => {
            eprintln!("{}", result.stderr.trim());
            std::process::exit(result.exit_code);
        }
        Err(error) => {
            eprintln!("failed to execute jj: {error}");
            std::process::exit(1);
        }
    }
}

pub fn print_help() {
    println!("vijutsu");
    println!();
    println!("Commands:");
    println!(
        "  inspect [--op <OP>] [--revset <REVSET>] [--limit <N>] [--change <ID>] [--compare <OP>]"
    );
    println!("  preview undo");
    println!("  preview restore <OP>");
    println!("  preview revert <OP>");
    println!("  preview abandon <OP>...");
    println!("  preview redo");
    println!("  mutate undo --execute");
    println!("  mutate restore <OP> --execute");
    println!("  mutate revert <OP> --execute");
    println!("  mutate redo --execute");
    println!("  snapshot [--execute]");
    println!("  heads");
    println!("  bookmarks");
    println!("  keys");
    println!("  tui");
    println!("  poll [--interval-ms <N>] [--count <N>]");
    println!("  capabilities");
    println!("  version");
}

pub fn print_heads() {
    let version = jj_version_string();
    let graph = fetch_operation_graph(&version, "@", 100, "operation heads");
    print!("{}", render_multiple_heads(&detect_multiple_heads(&graph)));
}

pub fn print_bookmarks() {
    match fetch_bookmark_list() {
        Ok(result) if result.exit_code == 0 => print!("{}", result.stdout),
        Ok(result) => {
            eprintln!("{}", result.stderr.trim());
            std::process::exit(result.exit_code);
        }
        Err(error) => fail_io("fetch bookmarks", error),
    }
}

pub fn print_keys() {
    print!("{}", render_keys());
}

pub fn poll(args: Vec<String>) {
    let options = match PollOptions::parse(&args) {
        Ok(options) => options,
        Err(error) => {
            eprintln!("{error}");
            std::process::exit(2);
        }
    };
    let version = jj_version_string();
    let mut state = WatcherState::default();

    for index in 0..options.count {
        let graph = fetch_operation_graph(&version, "@", 100, "poll operation heads");
        let heads = graph.head_ids.clone();
        let view = detect_multiple_heads(&graph);
        println!(
            "poll {}: heads={} {}",
            index + 1,
            view.head_count,
            view.head_ids.join(" ")
        );

        if state.last_known_heads.is_empty() {
            println!("baseline recorded");
        } else if let Some(event) = detect_operation_head_change(&state, heads.clone()) {
            state.pending_refresh = true;
            println!(
                "operation head changed: {} from {}; refresh needed",
                event.event_type, event.source
            );
        } else {
            println!("no operation head change");
        }

        update_watcher_state(&mut state, heads);
        if index + 1 < options.count {
            std::thread::sleep(Duration::from_millis(options.interval_ms));
        }
    }
}

pub fn preview(args: Vec<String>) {
    let Some(kind) = args.first().map(String::as_str) else {
        eprintln!("missing preview kind");
        print_help();
        std::process::exit(2);
    };
    match kind {
        "restore" => preview_restore(args.get(1).unwrap_or_else(|| missing_arg("restore <OP>"))),
        "revert" => preview_revert(args.get(1).unwrap_or_else(|| missing_arg("revert <OP>"))),
        "abandon" => preview_abandon(&args[1..]),
        "undo" => preview_undo(),
        "redo" => preview_redo(),
        other => {
            eprintln!("unknown preview kind: {other}");
            std::process::exit(2);
        }
    }
}

pub fn mutate(args: Vec<String>) {
    let Some(kind) = args.first().map(String::as_str) else {
        eprintln!("missing mutate kind");
        print_help();
        std::process::exit(2);
    };
    let execute = args.iter().any(|arg| arg == "--execute");
    if !execute {
        eprintln!("mutation requires --execute after preview");
        std::process::exit(2);
    }

    let extra_args = args[1..]
        .iter()
        .filter(|arg| arg.as_str() != "--execute")
        .collect::<Vec<_>>();
    let caps = detect_capabilities().unwrap_or_else(|error| fail_io("detect capabilities", error));
    require_mutation_protocol_capabilities(&caps).unwrap_or_else(|error| fail_message(&error));
    let version = jj_version_string();
    let preview_operation = current_operation_id(&version);
    let command_args = match kind {
        "undo" if extra_args.is_empty() => vec!["undo".to_string()],
        "redo" if extra_args.is_empty() => {
            if !caps.redo {
                eprintln!("jj redo is not supported by this Jujutsu version.");
                std::process::exit(1);
            }
            vec!["redo".to_string()]
        }
        "restore" if extra_args.len() == 1 => {
            if !caps.op_restore {
                eprintln!("jj op restore is not supported by this Jujutsu version.");
                std::process::exit(1);
            }
            vec![
                "op".to_string(),
                "restore".to_string(),
                extra_args[0].to_string(),
            ]
        }
        "revert" if extra_args.len() == 1 => {
            if !caps.op_revert {
                eprintln!("jj op revert is not supported by this Jujutsu version.");
                std::process::exit(1);
            }
            vec![
                "op".to_string(),
                "revert".to_string(),
                extra_args[0].to_string(),
            ]
        }
        "abandon" => {
            eprintln!("operation abandon is preview-only in this MVP");
            std::process::exit(2);
        }
        _ => {
            eprintln!("invalid mutate arguments");
            print_help();
            std::process::exit(2);
        }
    };

    let current_operation = current_operation_id(&version);
    guard_execution_head(&preview_operation, &current_operation).unwrap_or_else(|error| {
        eprintln!("{}", error.reason);
        eprintln!("Preview was based on op {}.", short(&preview_operation));
        eprintln!("Current head is now {}.", short(&current_operation));
        std::process::exit(1);
    });
    let result =
        execute_mutation(command_args).unwrap_or_else(|error| fail_io("execute mutation", error));
    print_command_result(&result);
    if result.exit_code != 0 {
        std::process::exit(result.exit_code);
    }
    println!("operation heads after mutation:");
    print_heads();
}

pub fn snapshot(args: Vec<String>) {
    let execute = args.iter().any(|arg| arg == "--execute");
    if args.iter().any(|arg| arg != "--execute") {
        eprintln!("unknown snapshot option");
        std::process::exit(2);
    }

    let caps = detect_capabilities().unwrap_or_else(|error| fail_io("detect capabilities", error));
    if !caps.util_snapshot {
        eprintln!("jj util snapshot is not supported by this Jujutsu version.");
        std::process::exit(1);
    }

    let warnings = snapshot_refresh_warnings();
    let preview = snapshot_refresh_result(true);
    if !execute {
        print!(
            "{}",
            render_preview_panel(&preview_panel_for_snapshot_refresh(&preview), &warnings)
        );
        return;
    }

    for warning in &warnings {
        eprintln!("warning: {warning}");
    }
    let result = execute_snapshot_refresh()
        .unwrap_or_else(|error| fail_io("execute snapshot refresh", error));
    println!("snapshot refresh executed: {}", result.command_display);
}

pub fn preview_restore(target_op: &str) {
    let version = jj_version_string();
    let current_op = current_operation_id(&version);
    let current_graph = fetch_change_graph(&version, &current_op, "all()");
    let target_graph = fetch_change_graph(&version, target_op, "all()");
    let delta = compute_operation_delta(target_op, &current_op, &target_graph, &current_graph);
    let preview = RestorePreview {
        target_operation: target_op.to_string(),
        current_graph,
        target_graph,
        delta,
        warnings: restore_warnings(),
    };
    print!(
        "{}",
        render_preview_panel(&preview_panel_for_restore(&preview), &preview.warnings)
    );
}

pub fn preview_undo() {
    let version = jj_version_string();
    let current = current_operation(&version);
    let Some(target_op) = current.parent_operation_ids.first().cloned() else {
        eprintln!("current operation has no parent to undo to");
        std::process::exit(1);
    };
    let current_graph = fetch_change_graph(&version, &current.operation_id, "all()");
    let target_graph = fetch_change_graph(&version, &target_op, "all()");
    let inverse_delta = compute_operation_delta(
        &current.operation_id,
        &target_op,
        &current_graph,
        &target_graph,
    );
    let preview = UndoPreview {
        target_operation: target_op,
        inverse_delta,
        new_operation_will_be_added: true,
        warnings: undo_warnings(),
    };
    print!(
        "{}",
        render_preview_panel(&preview_panel_for_undo(&preview), &preview.warnings)
    );
}

pub fn preview_revert(target_op: &str) {
    let version = jj_version_string();
    let current_op = current_operation_id(&version);
    let current_graph = fetch_change_graph(&version, &current_op, "all()");
    let target_graph = fetch_change_graph(&version, target_op, "all()");
    let inverse_effect =
        compute_operation_delta(target_op, &current_op, &target_graph, &current_graph);
    let preview = RevertPreview {
        target_operation: target_op.to_string(),
        inverse_effect,
        warnings: vec![
            "Restore recovers the entire view at the target operation.".to_string(),
            "Revert applies the inverse of one operation to the current state.".to_string(),
            "A new revert operation will be created.".to_string(),
        ],
    };
    print!(
        "{}",
        render_preview_panel(&preview_panel_for_revert(&preview), &preview.warnings)
    );
}

pub fn preview_abandon(target_ops: &[String]) {
    if target_ops.is_empty() {
        eprintln!("missing operation for preview abandon");
        std::process::exit(2);
    }
    let version = jj_version_string();
    let graph = fetch_operation_graph(&version, "@", 100, "preview abandon operation graph");
    let known_operation_ids = graph
        .nodes
        .iter()
        .map(|record| record.operation_id.as_str())
        .collect::<Vec<_>>();
    let resolved_targets = target_ops
        .iter()
        .map(|target| {
            resolve_operation_prefix(target, &known_operation_ids)
                .unwrap_or_else(|error| fail_message(&error))
        })
        .collect::<Vec<_>>();
    let current_operation = graph
        .nodes
        .iter()
        .find(|operation| operation.is_selected)
        .or_else(|| graph.nodes.first())
        .map(|operation| operation.operation_id.as_str());
    let preview = abandon_preview(resolved_targets, current_operation);
    print!(
        "{}",
        render_preview_panel(&preview_panel_for_abandon(&preview), &preview.warnings)
    );
}

pub fn preview_redo() {
    let caps = detect_capabilities().unwrap_or_else(|error| fail_io("detect capabilities", error));
    let preview = redo_preview(caps.redo);
    let warnings = preview
        .unavailable_reason
        .iter()
        .cloned()
        .collect::<Vec<_>>();
    print!(
        "{}",
        render_preview_panel(&preview_panel_for_redo(&preview), &warnings)
    );
}

pub fn print_capabilities() {
    let caps = detect_capabilities().unwrap_or_else(|error| fail_io("detect capabilities", error));
    println!("jj: {}", caps.version);
    println!("json_template_function: {}", caps.json_template_function);
    println!("escape_json_method: {}", caps.escape_json_method);
    println!(
        "operation_parent_template: {}",
        caps.operation_parent_template
    );
    println!(
        "commit_predecessor_template: {}",
        caps.commit_predecessor_template
    );
    println!("op_diff: {}", caps.op_diff);
    println!("op_show: {}", caps.op_show);
    println!("op_revert: {}", caps.op_revert);
    println!("op_restore: {}", caps.op_restore);
    println!("op_restore_what: {}", caps.op_restore_what);
    println!("redo: {}", caps.redo);
    println!("util_snapshot: {}", caps.util_snapshot);
    println!("no_integrate_operation: {}", caps.no_integrate_operation);
}

pub fn print_command_result(result: &crate::cli::CommandResult) {
    println!("command: {}", result.command_display);
    println!("exit: {}", result.exit_code);
    if !result.stdout.trim().is_empty() {
        println!("stdout:\n{}", result.stdout.trim());
    }
    if !result.stderr.trim().is_empty() {
        eprintln!("stderr:\n{}", result.stderr.trim());
    }
}

pub fn require_mutation_protocol_capabilities(caps: &JjCapabilities) -> Result<(), String> {
    let missing = [
        (!caps.json_template_function).then_some("json() template"),
        (!caps.escape_json_method).then_some("escape_json() template"),
        (!caps.operation_parent_template).then_some("operation parent template"),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();

    if missing.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "mutation disabled: unsupported jj protocol capability: {}",
            missing.join(", ")
        ))
    }
}

pub fn jj_version_string() -> String {
    let result = jj_version().unwrap_or_else(|error| fail_io("run jj version", error));
    if result.exit_code != 0 {
        eprintln!("{}", result.stderr.trim());
        std::process::exit(result.exit_code);
    }
    result.stdout.trim().to_string()
}

pub fn current_operation_id(version: &str) -> String {
    current_operation(version).operation_id
}

pub fn current_operation(version: &str) -> crate::core::OperationNode {
    let graph = fetch_operation_graph(version, "@", 1, "current operation");
    graph
        .nodes
        .into_iter()
        .next()
        .unwrap_or_else(|| fail_message("current operation not found"))
}

pub fn fetch_change_graph(version: &str, at_op: &str, revset: &str) -> crate::core::ChangeGraph {
    let parse = fetch_change_records(version, at_op, revset)
        .unwrap_or_else(|error| fail_io("fetch preview change graph", error));
    fail_parse_result("preview change graph", &parse);
    change_graph_from_parse(parse)
}

pub fn fetch_operation_graph(
    version: &str,
    at_op: &str,
    limit: usize,
    context: &str,
) -> crate::core::OperationGraph {
    let parse = fetch_operation_records(version, at_op, limit)
        .unwrap_or_else(|error| fail_io(context, error));
    fail_parse_result(context, &parse);
    operation_graph_from_parse(parse)
}

pub fn missing_arg(message: &str) -> ! {
    eprintln!("missing argument: {message}");
    std::process::exit(2);
}

pub fn fail_message(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

pub fn fail_parse_result<T>(context: &str, result: &ParseResult<T>) {
    fail_parse(context, &result.errors, &result.raw_stdout);
}

pub fn fail_parse(context: &str, errors: &[ParseError], raw_stdout: &str) {
    let Some(diagnostic) = parse_diagnostic(context, errors, raw_stdout) else {
        return;
    };

    eprint!("{diagnostic}");
    std::process::exit(1);
}

pub fn parse_diagnostic(context: &str, errors: &[ParseError], raw_stdout: &str) -> Option<String> {
    if errors.is_empty() {
        return None;
    }

    let mut out = format!("failed to parse {context}:\n");
    for error in errors.iter().take(5) {
        out.push_str(&format!(
            "line {}: {}: {}\n",
            error.line_number, error.message, error.raw_line
        ));
    }
    out.push_str("raw stdout preview:\n");
    for line in raw_stdout.lines().take(20) {
        out.push_str(line);
        out.push('\n');
    }
    Some(out)
}

pub fn fail_io(context: &str, error: std::io::Error) -> ! {
    eprintln!("failed to {context}: {error}");
    std::process::exit(1);
}

pub struct InspectOptions {
    selected_operation: Option<String>,
    revset: String,
    operation_limit: usize,
    selected_change: Option<String>,
    compare_operation: Option<String>,
}

impl InspectOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut revset = "all()".to_string();
        let mut operation_limit = 100;
        let mut selected_operation = None;
        let mut selected_change = None;
        let mut compare_operation = None;
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--revset" | "-r" => {
                    index += 1;
                    revset = args
                        .get(index)
                        .ok_or("missing value for --revset")?
                        .to_string();
                }
                "--limit" | "-n" => {
                    index += 1;
                    operation_limit = args
                        .get(index)
                        .ok_or("missing value for --limit")?
                        .parse()
                        .map_err(|_| "operation limit must be a positive integer")?;
                    if operation_limit == 0 {
                        return Err("operation limit must be a positive integer".to_string());
                    }
                }
                "--change" | "-c" => {
                    index += 1;
                    selected_change = Some(
                        args.get(index)
                            .ok_or("missing value for --change")?
                            .to_string(),
                    );
                }
                "--op" | "-o" => {
                    index += 1;
                    selected_operation =
                        Some(args.get(index).ok_or("missing value for --op")?.to_string());
                }
                "--compare" => {
                    index += 1;
                    compare_operation = Some(
                        args.get(index)
                            .ok_or("missing value for --compare")?
                            .to_string(),
                    );
                }
                other => return Err(format!("unknown inspect option: {other}")),
            }
            index += 1;
        }

        Ok(Self {
            selected_operation,
            revset,
            operation_limit,
            selected_change,
            compare_operation,
        })
    }
}

pub struct PollOptions {
    interval_ms: u64,
    count: usize,
}

impl PollOptions {
    fn parse(args: &[String]) -> Result<Self, String> {
        let mut interval_ms = 1000;
        let mut count = 2;
        let mut index = 0;

        while index < args.len() {
            match args[index].as_str() {
                "--interval-ms" => {
                    index += 1;
                    interval_ms = args
                        .get(index)
                        .ok_or("missing value for --interval-ms")?
                        .parse()
                        .map_err(|_| "interval must be a non-negative integer")?;
                }
                "--count" | "-n" => {
                    index += 1;
                    count = args
                        .get(index)
                        .ok_or("missing value for --count")?
                        .parse()
                        .map_err(|_| "count must be a positive integer")?;
                    if count == 0 {
                        return Err("count must be a positive integer".to_string());
                    }
                }
                other => return Err(format!("unknown poll option: {other}")),
            }
            index += 1;
        }

        Ok(Self { interval_ms, count })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| value.to_string()).collect()
    }

    #[test]
    fn poll_options_defaults_to_two_samples() {
        let options = PollOptions::parse(&[]).unwrap();

        assert_eq!(options.interval_ms, 1000);
        assert_eq!(options.count, 2);
    }

    #[test]
    fn inspect_options_accepts_operation_selection() {
        let options = InspectOptions::parse(&args(&["--op", "abc", "--revset", "all()"])).unwrap();

        assert_eq!(options.selected_operation.as_deref(), Some("abc"));
        assert_eq!(options.revset, "all()");
    }

    #[test]
    fn inspect_options_rejects_zero_limit() {
        assert!(InspectOptions::parse(&args(&["--limit", "0"])).is_err());
    }

    #[test]
    fn parse_diagnostic_includes_raw_stdout_preview() {
        let diagnostic = parse_diagnostic(
            "change graph",
            &[ParseError {
                line_number: 1,
                raw_line: "not json".to_string(),
                message: "expected value".to_string(),
            }],
            "not json\ntrailing text",
        )
        .unwrap();

        assert!(diagnostic.contains("failed to parse change graph"));
        assert!(diagnostic.contains("line 1: expected value: not json"));
        assert!(diagnostic.contains("raw stdout preview:"));
        assert!(diagnostic.contains("trailing text"));
    }

    #[test]
    fn mutation_protocol_gate_reports_missing_capabilities() {
        let mut caps = test_capabilities();
        assert!(require_mutation_protocol_capabilities(&caps).is_ok());

        caps.escape_json_method = false;
        let error = require_mutation_protocol_capabilities(&caps).unwrap_err();

        assert!(error.contains("mutation disabled"));
        assert!(error.contains("escape_json() template"));
    }

    #[test]
    fn poll_options_accepts_fast_single_sample() {
        let options = PollOptions::parse(&args(&["--interval-ms", "0", "--count", "1"])).unwrap();

        assert_eq!(options.interval_ms, 0);
        assert_eq!(options.count, 1);
    }

    #[test]
    fn poll_options_rejects_zero_count() {
        assert!(PollOptions::parse(&args(&["--count", "0"])).is_err());
    }

    fn test_capabilities() -> JjCapabilities {
        JjCapabilities {
            version: "test".to_string(),
            json_template_function: true,
            escape_json_method: true,
            operation_parent_template: true,
            commit_predecessor_template: false,
            op_diff: true,
            op_show: true,
            op_revert: true,
            op_restore: true,
            op_restore_what: true,
            redo: true,
            util_snapshot: true,
            no_integrate_operation: true,
        }
    }
}
