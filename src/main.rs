use std::collections::BTreeSet;
use std::io;
use std::process::Command;
use std::time::Duration;

use crossterm::event::{
    self, DisableMouseCapture, EnableMouseCapture, Event, KeyCode, KeyEventKind, KeyModifiers,
    MouseEventKind,
};
use crossterm::execute;
use crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
};
use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use vijutsu::cli::{
    ParseError, ParseResult, detect_capabilities, execute_snapshot_refresh, fetch_bookmark_list,
    fetch_change_records, fetch_diff, fetch_diff_for_path, fetch_file_summary,
    fetch_operation_records, jj_version,
};
use vijutsu::core::{JjCapabilities, ObservationFreshness};
use vijutsu::editor::{ConfiguredLauncher, OpenRequest, TerminalLauncher, load_editor_settings};
use vijutsu::graph::{GraphCharset, GraphNode, render_graph_rows};
use vijutsu::mutation::{
    RestorePreview, RevertPreview, UndoPreview, abandon_preview, execute_mutation, redo_preview,
    restore_warnings, snapshot_refresh_result, snapshot_refresh_warnings, undo_warnings,
};
use vijutsu::observation::{
    SelectionState, build_observation, change_graph_from_parse, compute_operation_delta,
    operation_graph_from_parse, resolve_change_selection, validate_head_consistency,
};
use vijutsu::state::{
    CommandKind, Freshness, ObservationContext, ViewMode, guard_command, guard_execution_head,
};
use vijutsu::state_fingerprint::{StateFingerprint, state_fingerprint, state_suffixes};
use vijutsu::tui::{
    ChangeDetails, DeltaDetails, KeyAction, UiCommand, UiPane, handle_key_input,
    preview_panel_for_abandon, preview_panel_for_redo, preview_panel_for_restore,
    preview_panel_for_revert, preview_panel_for_snapshot_refresh, preview_panel_for_undo,
    render_keys, render_multiple_heads, render_observation, render_preview_panel,
};
use vijutsu::tui_runtime::{focused_block, selectable_list, styled_lines, visible_from};
use vijutsu::watcher::{
    WatcherState, detect_filesystem_change, detect_multiple_heads, detect_operation_head_change,
    scan_filesystem, update_watcher_state,
};

fn main() {
    let mut args = std::env::args().skip(1);
    match args.next().as_deref() {
        Some("inspect") | None => inspect(args.collect()),
        Some("preview") => preview(args.collect()),
        Some("mutate") => mutate(args.collect()),
        Some("snapshot") => snapshot(args.collect()),
        Some("heads") => print_heads(),
        Some("bookmarks") => print_bookmarks(),
        Some("keys") => print_keys(),
        Some("tui") => run_tui_or_exit(),
        Some("poll") => poll(args.collect()),
        Some("capabilities") => print_capabilities(),
        Some("version") => print_version(),
        Some("help") | Some("--help") | Some("-h") => print_help(),
        Some(other) => {
            eprintln!("unknown command: {other}");
            print_help();
            std::process::exit(2);
        }
    }
}

fn inspect(args: Vec<String>) {
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

fn print_version() {
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

fn print_help() {
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

fn print_heads() {
    let version = jj_version_string();
    let graph = fetch_operation_graph(&version, "@", 100, "operation heads");
    print!("{}", render_multiple_heads(&detect_multiple_heads(&graph)));
}

fn print_bookmarks() {
    match fetch_bookmark_list() {
        Ok(result) if result.exit_code == 0 => print!("{}", result.stdout),
        Ok(result) => {
            eprintln!("{}", result.stderr.trim());
            std::process::exit(result.exit_code);
        }
        Err(error) => fail_io("fetch bookmarks", error),
    }
}

fn print_keys() {
    print!("{}", render_keys());
}

struct TuiApp {
    mode: UiMode,
    pane: UiPane,
    operations: Vec<TuiOperation>,
    changes: Vec<TuiChange>,
    files: Vec<TuiFile>,
    detail_title: String,
    detail: Vec<String>,
    overlay: Option<OverlayPane>,
    maximized_pane: Option<UiPane>,
    selected_operation: usize,
    selected_change: usize,
    selected_file: usize,
    selected_files: BTreeSet<usize>,
    file_selection_anchor: Option<usize>,
    operation_scroll: usize,
    operation_visible_rows: usize,
    change_scroll: usize,
    change_visible_rows: usize,
    file_scroll: usize,
    file_visible_rows: usize,
    detail_scroll: u16,
    version: String,
    current_head: String,
    observed_at: u64,
    status: String,
    command_log: Vec<CommandLogEntry>,
    pinned_change: Option<SelectedChange>,
    pending_mutation: Option<PendingMutation>,
    describe_input: Option<DescribeInput>,
    commit_input: Option<CommitInput>,
    bookmark_input: Option<BookmarkInput>,
    rebase_source: Option<SelectedChange>,
}

struct OverlayPane {
    title: String,
    lines: Vec<String>,
    scroll: u16,
}

struct InputCursor {
    line: u16,
    column: u16,
}

struct CommandLogEntry {
    command: String,
    exit_code: Option<i32>,
    status: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum UiMode {
    Browse,
    Surgery,
}

struct TuiOperation {
    id: String,
    label: String,
    parent_ids: Vec<String>,
    is_current: bool,
    is_head: bool,
}

struct TuiChange {
    change_id: String,
    commit_id: String,
    label: String,
    parent_commit_ids: Vec<String>,
    bookmarks: Vec<String>,
    is_workspace: bool,
}

struct TuiFile {
    path: String,
    label: String,
}

#[derive(Debug, Clone)]
enum PendingMutation {
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
struct SelectedChange {
    change_id: String,
    commit_id: String,
}

struct DescribeInput {
    change_id: String,
    commit_id: String,
    message: String,
}

struct CommitInput {
    message: String,
}

struct BookmarkInput {
    change: SelectedChange,
    name: String,
}

impl PendingMutation {
    fn name(&self) -> &'static str {
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

    fn command_args(&self) -> Vec<String> {
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

    fn preferred_change_id(&self) -> Option<&str> {
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

    fn command_kind(&self) -> CommandKind {
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

    fn preview_lines(&self) -> Vec<String> {
        let args = self.command_args();
        let mut lines = match self {
            PendingMutation::Undo => vec![
                "Undo last operation".to_string(),
                "A new operation will be created.".to_string(),
            ],
            PendingMutation::Edit { change } => vec![
                format!("Edit {}", change.brief()),
                String::new(),
                "e  Edit selected change directly".to_string(),
                "n  Create a new child change".to_string(),
                String::new(),
                "Working copy moves to selected change.".to_string(),
                "Its descendants may be rebased as you edit it.".to_string(),
            ],
            PendingMutation::NewChild { change } => vec![
                format!("New child from {}", change.brief()),
                String::new(),
                "e  Edit selected change directly".to_string(),
                "n  Create a new child change".to_string(),
                String::new(),
                "Working copy moves to a new child of the selected change.".to_string(),
            ],
            PendingMutation::SquashCurrent {
                source,
                destination,
            } => vec![
                "Squash current into parent".to_string(),
                String::new(),
                format!("source: {}", source.brief()),
                format!("destination: {}", destination.brief()),
                String::new(),
                "Before             After".to_string(),
                String::new(),
                "@ C                @ empty".to_string(),
                "│                  │".to_string(),
                "○ B                ○ B+C".to_string(),
            ],
            PendingMutation::Abandon {
                change,
                descendants,
                bookmarks,
            } => vec![
                format!("Abandon {}", change.brief()),
                String::new(),
                "Before             After".to_string(),
                String::new(),
                "@ C                @ C'".to_string(),
                "│                  │".to_string(),
                "○ B       ->       ○ A".to_string(),
                "│".to_string(),
                "○ A".to_string(),
                String::new(),
                "Abandoned:".to_string(),
                format!("  {}", change.brief()),
                String::new(),
                "Descendants rebased:".to_string(),
                format!("  {}", non_empty_join(descendants, "(none)")),
                String::new(),
                "Bookmarks affected:".to_string(),
                format!("  {}", non_empty_join(bookmarks, "(none)")),
                String::new(),
                "Undo available:".to_string(),
                "  y".to_string(),
            ],
            PendingMutation::Rebase {
                source,
                destination,
            } => vec![
                "Rebase".to_string(),
                String::new(),
                format!("source: {}", source.brief()),
                format!("destination: {}", destination.brief()),
                String::new(),
                "The selected revision is moved onto the destination.".to_string(),
                "Descendants are not moved by this command.".to_string(),
            ],
            PendingMutation::Restore { change } => vec![
                format!("Restore changes in {}", change.brief()),
                String::new(),
                "This undoes the selected change's diff in the working copy.".to_string(),
                "It is equivalent to jj restore --changes-in <selected>.".to_string(),
                "Use U to undo this operation if it was not intended.".to_string(),
            ],
            PendingMutation::ImportHistoricalFile {
                source_op,
                source,
                paths,
            } => vec![
                "Import historical file into current working copy".to_string(),
                String::new(),
                format!("source op: {}", short(source_op)),
                format!("source commit: {}", short(&source.commit_id)),
                format!("source paths: {}", paths.join(", ")),
                "write target: current operation head (@)".to_string(),
                String::new(),
                "The historical operation remains read-only.".to_string(),
                "This writes only to the current working copy.".to_string(),
            ],
            PendingMutation::Commit { message } => vec![
                "Commit current working-copy change".to_string(),
                String::new(),
                "This updates the current change description and creates a new child change."
                    .to_string(),
                "It is equivalent to jj commit -m <message>.".to_string(),
                String::new(),
                "Message:".to_string(),
                format!("  {message}"),
            ],
            PendingMutation::BookmarkSet { name, change } => vec![
                "Set bookmark".to_string(),
                String::new(),
                format!("Bookmark: {name}"),
                format!("Target: {}", change.brief()),
                String::new(),
                "This creates or moves the local bookmark to the selected change.".to_string(),
                "Push later publishes the bookmark position to a Git remote.".to_string(),
            ],
            PendingMutation::Push { bookmark } => {
                let mut lines = vec![
                    "Push to Git remote".to_string(),
                    String::new(),
                    "This runs jj git push with the repository's default remote/bookmark behavior."
                        .to_string(),
                    "It may update remote bookmarks after Jujutsu safety checks.".to_string(),
                    "If this is not intended, press N or Esc.".to_string(),
                ];
                if let Some(bookmark) = bookmark {
                    lines.insert(2, format!("Bookmark: {bookmark}"));
                    lines.insert(3, String::new());
                }
                lines
            }
        };
        lines.push(String::new());
        lines.push(format!("Command: jj {}", args.join(" ")));
        lines.push("Press y to execute, Esc or N to cancel.".to_string());
        lines
    }
}

impl SelectedChange {
    fn brief(&self) -> String {
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
    fn brief(&self) -> String {
        format!("{} {}", short(&self.change_id), short(&self.commit_id))
    }

    fn local_bookmark(&self) -> Option<String> {
        self.bookmarks
            .iter()
            .find(|bookmark| !bookmark.contains('@'))
            .cloned()
    }
}

impl TuiApp {
    fn new() -> Self {
        let mut app = Self {
            mode: UiMode::Browse,
            pane: UiPane::Operation,
            operations: Vec::new(),
            changes: Vec::new(),
            files: Vec::new(),
            detail_title: "4 Detail".to_string(),
            detail: vec![
                "Enter: show focused item here".to_string(),
                "u: undo preview  Ctrl-r: redo preview  R: snapshot preview".to_string(),
            ],
            overlay: None,
            maximized_pane: None,
            selected_operation: 0,
            selected_change: 0,
            selected_file: 0,
            selected_files: BTreeSet::new(),
            file_selection_anchor: None,
            operation_scroll: 0,
            operation_visible_rows: 1,
            change_scroll: 0,
            change_visible_rows: 1,
            file_scroll: 0,
            file_visible_rows: 1,
            detail_scroll: 0,
            version: String::new(),
            current_head: "?".to_string(),
            observed_at: 0,
            status: "browse mode; mutations require y confirmation".to_string(),
            command_log: Vec::new(),
            pinned_change: None,
            pending_mutation: None,
            describe_input: None,
            commit_input: None,
            bookmark_input: None,
            rebase_source: None,
        };
        app.refresh_all();
        app
    }

    fn refresh_all(&mut self) {
        self.observed_at = self.observed_at.saturating_add(1);
        match load_operations() {
            Ok((version, current_head, operations)) => {
                self.version = version;
                self.current_head = current_head;
                self.operations = operations;
                self.selected_operation =
                    clamp_index(self.selected_operation, self.operations.len());
                self.refresh_changes();
                self.set_browse_status(format!(
                    "gen {} sync; stale async results: none",
                    self.observed_at
                ));
            }
            Err(error) => self.status = error,
        }
    }

    fn refresh_after_mutation(&mut self, preferred_change_id: Option<&str>) {
        self.refresh_all();
        if let Some(index) = self
            .operations
            .iter()
            .position(|operation| operation.is_current)
        {
            self.selected_operation = index;
            self.refresh_changes();
        }
        if let Some(change_id) = preferred_change_id
            && let Some(index) = self
                .changes
                .iter()
                .position(|change| change.change_id == change_id)
        {
            self.selected_change = index;
            self.refresh_files();
        }
    }

    fn refresh_changes(&mut self) {
        self.changes.clear();
        self.files.clear();
        let Some(op_id) = self.selected_operation_id() else {
            return;
        };
        match load_changes(&self.version, &op_id) {
            Ok(changes) => {
                self.changes = changes;
                self.selected_change = 0;
                self.refresh_files();
            }
            Err(error) => self.status = error,
        }
    }

    fn refresh_files(&mut self) {
        self.files.clear();
        self.selected_file = 0;
        self.selected_files.clear();
        self.file_selection_anchor = None;
        let Some(op_id) = self.selected_operation_id() else {
            return;
        };
        let Some(commit_id) = self.selected_commit_id() else {
            return;
        };
        if is_root_commit(&commit_id) {
            self.files = vec![TuiFile {
                path: String::new(),
                label: "ROOT has no file diff".to_string(),
            }];
            self.select_file(0, false, false);
            self.detail = vec!["ROOT [immutable]".to_string()];
            self.detail_title = "ROOT".to_string();
            return;
        }
        match fetch_file_summary(&op_id, &commit_id) {
            Ok(files) => {
                self.files = if files.is_empty() {
                    vec![TuiFile {
                        path: String::new(),
                        label: "(no files)".to_string(),
                    }]
                } else {
                    files
                        .into_iter()
                        .map(|file| TuiFile {
                            label: format!("{} {}", file.status, file.path),
                            path: file.path,
                        })
                        .collect()
                };
                self.selected_file = 0;
                self.file_scroll = 0;
                self.select_file(0, false, false);
                self.refresh_diff();
            }
            Err(error) => self.status = format!("failed to fetch files: {error}"),
        }
    }

    fn refresh_diff(&mut self) {
        let Some(op_id) = self.selected_operation_id() else {
            return;
        };
        let Some(commit_id) = self.selected_commit_id() else {
            return;
        };
        if is_root_commit(&commit_id) {
            return;
        }
        let path = self
            .files
            .get(self.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("");
        let diff = if path.is_empty() {
            fetch_diff(&op_id, &commit_id)
        } else {
            fetch_diff_for_path(&op_id, &commit_id, path)
        };
        match diff {
            Ok(diff) => {
                self.detail_title = if path.is_empty() {
                    format!("4 Diff {}", short(&commit_id))
                } else {
                    format!("4 Diff {} {}", short(&commit_id), path)
                };
                self.detail =
                    non_empty_or(diff.lines().map(str::to_string).collect(), "(empty diff)");
                self.detail_scroll = 0;
            }
            Err(error) => self.status = format!("failed to fetch diff: {error}"),
        }
    }

    fn selected_operation_id(&self) -> Option<String> {
        self.operations
            .get(self.selected_operation)
            .map(|operation| operation.id.clone())
    }

    fn current_operation(&self) -> Option<&TuiOperation> {
        self.operations
            .iter()
            .find(|operation| operation.is_current)
    }

    fn is_viewing_current_operation(&self) -> bool {
        self.operations
            .get(self.selected_operation)
            .map(|operation| operation.is_current)
            .unwrap_or(false)
    }

    fn view_mode(&self) -> &'static str {
        if self.is_viewing_current_operation() {
            "current"
        } else {
            "historical"
        }
    }

    fn historical_warning(&self) -> Option<String> {
        if self.is_viewing_current_operation() {
            return None;
        }
        Some(format!(
            "HISTORICAL VIEW - mutations disabled | current op {} | viewing op {}",
            self.current_operation()
                .map(|operation| short(&operation.id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.selected_operation_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string())
        ))
    }

    fn observation_context(&self) -> ObservationContext {
        let selected = self
            .selected_operation_id()
            .unwrap_or_else(|| "?".to_string());
        let current = self
            .current_operation()
            .map(|operation| operation.id.clone())
            .unwrap_or_else(|| self.current_head.clone());
        ObservationContext {
            view_mode: if self.is_viewing_current_operation() {
                ViewMode::FollowCurrent
            } else {
                ViewMode::Historical(selected.clone())
            },
            observed_operation: selected,
            current_operation: current,
            freshness: Freshness::Current,
        }
    }

    fn operation_context_lines(&self) -> Vec<String> {
        let current = self
            .current_operation()
            .map(|operation| short(&operation.id).to_string())
            .unwrap_or_else(|| "?".to_string());
        let viewing = self
            .selected_operation_id()
            .map(|id| short(&id).to_string())
            .unwrap_or_else(|| "?".to_string());
        vec![
            format!("current op: {current}"),
            format!("viewing op: {viewing} ({})", self.view_mode()),
        ]
    }

    fn block_historical_mutation(&mut self, action: &str) -> bool {
        self.block_command(action, CommandKind::MutateCurrent)
    }

    fn block_command(&mut self, action: &str, kind: CommandKind) -> bool {
        let Err(error) = guard_command(&self.observation_context(), kind) else {
            return false;
        };
        let mut lines = self.operation_context_lines();
        lines.push(String::new());
        lines.push(format!("{action} is blocked: {}.", error.reason));
        lines.push(
            "Normal mutations require the current operation; historical views can only import into current."
                .to_string(),
        );
        lines.push(
            "OP pane Enter only shows operation details; it does not change mutation target."
                .to_string(),
        );
        self.show_overlay("Historical View", lines);
        self.status = self
            .historical_warning()
            .unwrap_or_else(|| "historical view: mutation blocked".to_string());
        true
    }

    fn show_overlay(&mut self, title: impl Into<String>, lines: Vec<String>) {
        self.overlay = Some(OverlayPane {
            title: title.into(),
            lines,
            scroll: 0,
        });
    }

    fn clear_overlay(&mut self) {
        self.overlay = None;
    }

    fn scroll_overlay_page(&mut self, delta: i16) -> bool {
        let Some(overlay) = &mut self.overlay else {
            return false;
        };
        if delta < 0 {
            overlay.scroll = overlay.scroll.saturating_sub(delta.unsigned_abs());
        } else {
            overlay.scroll = overlay.scroll.saturating_add(delta as u16);
        }
        self.status = "overlay scrolled".to_string();
        true
    }

    fn set_browse_status(&mut self, status: impl Into<String>) {
        self.status = self.historical_warning().unwrap_or_else(|| status.into());
    }

    fn selected_commit_id(&self) -> Option<String> {
        self.changes
            .get(self.selected_change)
            .map(|change| change.commit_id.clone())
    }

    fn selected_change(&self) -> Option<SelectedChange> {
        self.changes
            .get(self.selected_change)
            .map(SelectedChange::from)
    }

    fn workspace_change(&self) -> Option<SelectedChange> {
        self.changes
            .iter()
            .find(|change| change.is_workspace)
            .map(SelectedChange::from)
    }

    fn change_by_commit(&self, commit_id: &str) -> Option<SelectedChange> {
        self.changes
            .iter()
            .find(|change| change.commit_id == commit_id)
            .map(SelectedChange::from)
    }

    fn descendants_of(&self, commit_id: &str) -> Vec<String> {
        let mut pending = vec![commit_id.to_string()];
        let mut descendants = Vec::new();
        while let Some(parent) = pending.pop() {
            for change in self
                .changes
                .iter()
                .filter(|change| change.parent_commit_ids.iter().any(|id| id == &parent))
            {
                if descendants
                    .iter()
                    .any(|id: &String| id == &change.commit_id)
                {
                    continue;
                }
                descendants.push(change.brief());
                pending.push(change.commit_id.clone());
            }
        }
        descendants
    }

    fn selected_file_path(&self) -> &str {
        self.files
            .get(self.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("")
    }

    fn context_spans(&self) -> Vec<Span<'_>> {
        let op = self
            .selected_operation_id()
            .map(|id| short(&id).to_string())
            .unwrap_or_else(|| "?".to_string());
        let (label, fg, bg) = self.mode_badge();
        vec![
            Span::styled(
                format!(" {label} "),
                Style::default().fg(fg).bg(bg).add_modifier(Modifier::BOLD),
            ),
            Span::raw(" "),
            Span::styled(
                format!(
                    "op {op} | current head {} | {} | snapshot only | fs unchanged | gen {}",
                    short(&self.current_head),
                    self.view_mode(),
                    self.observed_at
                ),
                Style::default().fg(Color::Cyan),
            ),
        ]
    }

    fn mode_badge(&self) -> (&'static str, Color, Color) {
        if self.pending_mutation.as_ref().is_some_and(|mutation| {
            matches!(mutation, PendingMutation::ImportHistoricalFile { .. })
        }) {
            ("IMPORT", Color::Black, Color::Yellow)
        } else if self.mode == UiMode::Surgery {
            ("SURGERY", Color::White, Color::Red)
        } else if self.is_viewing_current_operation() {
            ("CURRENT", Color::Black, Color::Green)
        } else {
            ("TIME TRAVEL", Color::Black, Color::Magenta)
        }
    }

    fn status_color(&self) -> Color {
        if self.pending_mutation.as_ref().is_some_and(|mutation| {
            matches!(mutation, PendingMutation::ImportHistoricalFile { .. })
        }) {
            Color::Yellow
        } else if self.mode == UiMode::Surgery {
            Color::Red
        } else if self.is_viewing_current_operation() {
            Color::Green
        } else {
            Color::Red
        }
    }

    fn operation_title(&self) -> String {
        format!(
            "1 Operation ref:{}",
            self.selected_operation_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string())
        )
    }

    fn change_title(&self) -> String {
        format!(
            "2 Change op:{} change:{} commit:{}",
            self.selected_operation_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.changes
                .get(self.selected_change)
                .map(|change| short(&change.change_id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.selected_commit_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string())
        )
    }

    fn files_title(&self) -> String {
        format!(
            "3 Files commit:{} file:{}",
            self.selected_commit_id()
                .map(|id| short(&id).to_string())
                .unwrap_or_else(|| "?".to_string()),
            self.selected_file_path()
        )
    }

    fn operation_labels(&self) -> Vec<String> {
        let charset = GraphCharset::from_env();
        render_graph_rows(
            &self
                .operations
                .iter()
                .map(|operation| GraphNode {
                    id: operation.id.clone(),
                    parent_ids: operation.parent_ids.clone(),
                    marker: if operation.is_current {
                        '@'
                    } else if operation.is_head {
                        match charset {
                            GraphCharset::Unicode => '◆',
                            GraphCharset::Ascii => '*',
                        }
                    } else {
                        match charset {
                            GraphCharset::Unicode => '○',
                            GraphCharset::Ascii => 'o',
                        }
                    },
                    text: operation.label.clone(),
                })
                .collect::<Vec<_>>(),
            charset,
        )
    }

    fn change_labels(&self) -> Vec<String> {
        let charset = GraphCharset::from_env();
        render_graph_rows(
            &self
                .changes
                .iter()
                .map(|change| GraphNode {
                    id: change.commit_id.clone(),
                    parent_ids: change.parent_commit_ids.clone(),
                    marker: if change.is_workspace {
                        '@'
                    } else {
                        match charset {
                            GraphCharset::Unicode => '○',
                            GraphCharset::Ascii => 'o',
                        }
                    },
                    text: change.label.clone(),
                })
                .collect::<Vec<_>>(),
            charset,
        )
    }

    fn selected_change_detail_lines(&self) -> Vec<String> {
        let Some(change) = self.changes.get(self.selected_change) else {
            return vec!["(nothing selected)".to_string()];
        };
        let mut lines = vec![
            selected_line(&self.change_labels(), self.selected_change),
            format!("change: {}", change.change_id),
            format!("commit: {}", change.commit_id),
        ];
        if !change.parent_commit_ids.is_empty() {
            lines.push(format!(
                "parents: {}",
                change
                    .parent_commit_ids
                    .iter()
                    .map(|id| short(id).to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        lines.push(String::new());
        lines.push("Bookmarks:".to_string());
        if change.bookmarks.is_empty() {
            lines.push("  (none)".to_string());
        } else {
            lines.extend(
                change
                    .bookmarks
                    .iter()
                    .map(|bookmark| format!("  {}", bookmark_display(bookmark))),
            );
        }
        lines
    }

    fn file_labels(&self) -> Vec<String> {
        self.files
            .iter()
            .enumerate()
            .map(|(index, file)| {
                if self.selected_files.contains(&index) {
                    format!("* {}", file.label)
                } else {
                    format!("  {}", file.label)
                }
            })
            .collect()
    }

    fn focus_previous(&mut self) {
        self.pane = match self.pane {
            UiPane::Operation | UiPane::Repository => UiPane::Diff,
            UiPane::Change => UiPane::Operation,
            UiPane::Files => UiPane::Change,
            UiPane::Diff => UiPane::Files,
        };
        self.set_browse_status(format!("pane: {:?}", self.pane));
    }

    fn focus_at(
        &mut self,
        column: u16,
        row: u16,
        width: u16,
        height: u16,
        modifiers: KeyModifiers,
    ) {
        if self.overlay.is_some() {
            return;
        }
        let body_height = height.saturating_sub(6);
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        let item_row = row.saturating_sub(1) as usize;
        self.pane = match pane_at(column, row, width, height) {
            Some(UiPane::Operation) => {
                self.selected_operation =
                    clamp_index(self.operation_scroll + item_row, self.operations.len());
                UiPane::Operation
            }
            Some(UiPane::Change) => {
                self.selected_change =
                    clamp_index(self.change_scroll + item_row, self.changes.len());
                UiPane::Change
            }
            Some(UiPane::Files) => {
                let files_top = body_height.saturating_mul(55) / 100;
                let row_in_files = row.saturating_sub(files_top).saturating_sub(1) as usize;
                let target = clamp_index(self.file_scroll + row_in_files, self.files.len());
                self.select_file(target, is_command_modifier(modifiers), false);
                UiPane::Files
            }
            Some(UiPane::Diff) => UiPane::Diff,
            _ => self.pane,
        };
        self.sync_after_selection(old_operation, old_change, old_file);
        self.set_browse_status(format!("pane: {:?}", self.pane));
    }

    fn scroll_at(&mut self, column: u16, row: u16, width: u16, height: u16, delta: isize) {
        if self.overlay.is_some() {
            self.scroll_overlay_page((delta * 3) as i16);
            return;
        }
        self.pane = pane_at(column, row, width, height).unwrap_or(self.pane);
        match self.pane {
            UiPane::Diff => self.scroll_detail_page((delta * 3) as i16),
            _ => self.move_selection(delta),
        }
    }

    fn sync_after_selection(&mut self, old_operation: usize, old_change: usize, old_file: usize) {
        if self.selected_operation != old_operation {
            self.refresh_changes();
            self.keep_selected_operation_visible();
        } else if self.selected_change != old_change {
            self.refresh_files();
            self.keep_selected_change_visible();
        } else if self.selected_file != old_file {
            self.refresh_diff();
        } else {
            return;
        }
        self.detail_scroll = 0;
        self.set_browse_status("selection updated");
    }

    fn move_selection(&mut self, delta: isize) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                self.selected_operation =
                    move_index(self.selected_operation, self.operations.len(), delta);
                self.keep_selected_operation_visible();
            }
            UiPane::Change => {
                self.selected_change = move_index(self.selected_change, self.changes.len(), delta);
                self.keep_selected_change_visible();
            }
            UiPane::Files => {
                self.selected_file = move_index(self.selected_file, self.files.len(), delta);
                self.select_file(self.selected_file, false, false);
                self.keep_selected_file_visible();
            }
            UiPane::Diff => {
                if delta < 0 {
                    self.detail_scroll = self.detail_scroll.saturating_sub(1);
                } else {
                    self.detail_scroll = self.detail_scroll.saturating_add(1);
                }
            }
        }
        self.sync_after_selection(old_operation, old_change, old_file);
    }

    fn jump_selection(&mut self, end: bool) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                self.selected_operation = if end {
                    self.operations.len().saturating_sub(1)
                } else {
                    0
                };
                self.keep_selected_operation_visible();
            }
            UiPane::Change => {
                self.selected_change = if end {
                    self.changes.len().saturating_sub(1)
                } else {
                    0
                };
                self.keep_selected_change_visible();
            }
            UiPane::Files => {
                let target = if end {
                    self.files.len().saturating_sub(1)
                } else {
                    0
                };
                self.select_file(target, false, false);
                self.keep_selected_file_visible();
            }
            UiPane::Diff => {}
        }
        self.sync_after_selection(old_operation, old_change, old_file);
    }

    fn move_graph_relative(&mut self, to_parent: bool) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                if let Some(index) = self.relative_operation_index(to_parent) {
                    self.selected_operation = index;
                    self.keep_selected_operation_visible();
                } else {
                    self.status = "no operation edge in that direction".to_string();
                    return;
                }
            }
            UiPane::Change => {
                if let Some(index) = self.relative_change_index(to_parent) {
                    self.selected_change = index;
                    self.keep_selected_change_visible();
                } else {
                    self.status = "no change edge in that direction".to_string();
                    return;
                }
            }
            _ => {
                self.status =
                    "h/l graph navigation works in Operation and Change panes".to_string();
                return;
            }
        }
        self.sync_after_selection(old_operation, old_change, old_file);
    }

    fn relative_operation_index(&self, to_parent: bool) -> Option<usize> {
        let selected = self.operations.get(self.selected_operation)?;
        if to_parent {
            let parent = selected.parent_ids.first()?;
            self.operations.iter().position(|op| &op.id == parent)
        } else {
            self.operations
                .iter()
                .position(|op| op.parent_ids.iter().any(|id| id == &selected.id))
        }
    }

    fn relative_change_index(&self, to_parent: bool) -> Option<usize> {
        let selected = self.changes.get(self.selected_change)?;
        if to_parent {
            let parent = selected.parent_commit_ids.first()?;
            self.changes
                .iter()
                .position(|change| &change.commit_id == parent)
        } else {
            self.changes.iter().position(|change| {
                change
                    .parent_commit_ids
                    .iter()
                    .any(|id| id == &selected.commit_id)
            })
        }
    }

    fn toggle_pin_comparison(&mut self) {
        let Some(current) = self.selected_change() else {
            self.status = "no change selected to pin".to_string();
            return;
        };
        if let Some(pinned) = self.pinned_change.take() {
            let lines = vec![
                "Pinned comparison".to_string(),
                String::new(),
                format!("pinned:  {}", pinned.brief()),
                format!("current: {}", current.brief()),
            ];
            self.show_overlay("Pinned Comparison", lines);
            self.status = "pin comparison shown".to_string();
        } else {
            self.pinned_change = Some(current.clone());
            self.status = format!("pinned {}", current.brief());
        }
    }

    fn extend_file_selection(&mut self, delta: isize) {
        let old_file = self.selected_file;
        let target = move_index(self.selected_file, self.files.len(), delta);
        self.select_file(target, false, true);
        self.keep_selected_file_visible();
        if self.selected_file != old_file {
            self.refresh_diff();
        }
        self.status = format!("{} file(s) selected", self.selected_files.len());
    }

    fn select_file(&mut self, index: usize, toggle: bool, extend: bool) {
        if self.files.is_empty() {
            self.selected_file = 0;
            self.selected_files.clear();
            self.file_selection_anchor = None;
            return;
        }
        let index = clamp_index(index, self.files.len());
        self.selected_file = index;
        if toggle {
            if !self.selected_files.insert(index) {
                self.selected_files.remove(&index);
            }
            self.file_selection_anchor = Some(index);
        } else if extend {
            let anchor = self.file_selection_anchor.unwrap_or(index);
            self.selected_files.clear();
            for item in anchor.min(index)..=anchor.max(index) {
                self.selected_files.insert(item);
            }
        } else {
            self.selected_files.clear();
            self.selected_files.insert(index);
            self.file_selection_anchor = Some(index);
        }
    }

    fn keep_selected_file_visible(&mut self) {
        keep_selected_visible(
            self.selected_file,
            &mut self.file_scroll,
            self.file_visible_rows,
        );
    }

    fn keep_selected_operation_visible(&mut self) {
        keep_selected_visible(
            self.selected_operation,
            &mut self.operation_scroll,
            self.operation_visible_rows,
        );
    }

    fn keep_selected_change_visible(&mut self) {
        keep_selected_visible(
            self.selected_change,
            &mut self.change_scroll,
            self.change_visible_rows,
        );
    }

    fn scroll_detail_page(&mut self, delta: i16) {
        if delta < 0 {
            self.detail_scroll = self.detail_scroll.saturating_sub(delta.unsigned_abs());
        } else {
            self.detail_scroll = self.detail_scroll.saturating_add(delta as u16);
        }
        self.status = "detail scrolled".to_string();
    }

    fn open_focused(&mut self) {
        let (title, lines) = match self.pane {
            UiPane::Operation | UiPane::Repository => {
                let mut lines = self.operation_context_lines();
                lines.push(String::new());
                lines.push(selected_line(
                    &self.operation_labels(),
                    self.selected_operation,
                ));
                lines.push(String::new());
                lines.push(
                    "Enter shows operation details only; it never changes mutation target."
                        .to_string(),
                );
                if !self.is_viewing_current_operation() {
                    lines.push("Mutations are disabled in historical views.".to_string());
                }
                ("4 Operation", lines)
            }
            UiPane::Change => ("4 Change", self.selected_change_detail_lines()),
            UiPane::Files => {
                self.clear_overlay();
                self.refresh_diff();
                self.pane = UiPane::Diff;
                self.set_browse_status("opened file diff");
                return;
            }
            UiPane::Diff => {
                self.scroll_detail_page(12);
                return;
            }
        };
        self.show_overlay(title.trim_start_matches("4 "), lines);
        self.set_browse_status(format!("opened {title} detail"));
    }

    fn set_preview(&mut self, title: &str, args: &[&str]) {
        let mut lines = run_self(args)
            .lines()
            .map(str::to_string)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push("(empty preview)".to_string());
        }
        self.show_overlay(title, lines);
        self.status = format!("rendered {title}");
    }

    fn show_help(&mut self) {
        self.show_overlay("Help", pane_mode_help());
        self.status = "help".to_string();
    }

    fn show_command_log(&mut self) {
        let mut lines = vec!["Recent commands".to_string(), String::new()];
        if self.command_log.is_empty() {
            lines.push("(no commands yet)".to_string());
        } else {
            lines.extend(self.command_log.iter().rev().map(|entry| {
                let exit = entry
                    .exit_code
                    .map(|code| code.to_string())
                    .unwrap_or_else(|| "-".to_string());
                format!("exit {exit} | {} | {}", entry.status, entry.command)
            }));
        }
        self.show_overlay("Command Log", lines);
        self.status = "command log".to_string();
    }

    fn push_command_log(&mut self, command: String, exit_code: Option<i32>, status: String) {
        self.command_log.push(CommandLogEntry {
            command,
            exit_code,
            status,
        });
        const MAX_COMMAND_LOG: usize = 30;
        if self.command_log.len() > MAX_COMMAND_LOG {
            self.command_log.remove(0);
        }
    }

    fn input_cursor(&self) -> Option<InputCursor> {
        if let Some(input) = &self.describe_input {
            return Some(InputCursor {
                line: 3,
                column: input.message.chars().count() as u16,
            });
        }
        if let Some(input) = &self.commit_input {
            return Some(InputCursor {
                line: 3,
                column: input.message.chars().count() as u16,
            });
        }
        if let Some(input) = &self.bookmark_input {
            return Some(InputCursor {
                line: 3,
                column: input.name.chars().count() as u16,
            });
        }
        None
    }

    fn toggle_maximize_pane(&mut self) {
        self.maximized_pane = match self.maximized_pane {
            Some(pane) if pane == self.pane => None,
            _ => Some(self.pane),
        };
        self.set_browse_status(match self.maximized_pane {
            Some(_) => "pane maximized",
            None => "pane restored",
        });
    }

    fn enter_surgery_mode(&mut self) {
        self.mode = UiMode::Surgery;
        self.pane = UiPane::Operation;
        self.status = "operation graph surgery; Operation pane actions only".to_string();
    }

    fn exit_surgery_mode(&mut self) {
        self.mode = UiMode::Browse;
        self.set_browse_status("left surgery mode");
    }

    fn preview_surgery_abandon_selected_operation(&mut self) {
        let Some(target) = self.selected_operation_id() else {
            self.status = "no operation selected".to_string();
            return;
        };
        let current = self
            .current_operation()
            .map(|operation| operation.id.as_str());
        let preview = abandon_preview(vec![target.clone()], current);
        let mut lines =
            render_preview_panel(&preview_panel_for_abandon(&preview), &preview.warnings)
                .lines()
                .map(str::to_string)
                .collect::<Vec<_>>();
        lines.insert(0, "direct execution blocked".to_string());
        lines.insert(0, "operation graph surgery preview".to_string());
        lines.insert(0, format!("target op: {}", short(&target)));
        self.show_overlay("Surgery preview abandon", lines);
        self.status = "surgery preview only; direct execution blocked".to_string();
    }

    fn plan_undo(&mut self) {
        if self.block_historical_mutation("undo") {
            return;
        }
        self.set_pending_mutation(PendingMutation::Undo);
    }

    fn plan_edit_selected(&mut self) {
        if self.block_historical_mutation("edit") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot edit ROOT".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Edit { change });
    }

    fn plan_new_child_selected(&mut self) {
        if self.block_historical_mutation("new child") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot create child from ROOT".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::NewChild { change });
    }

    fn plan_squash_current(&mut self) {
        if self.block_historical_mutation("squash") {
            return;
        }
        let Some(source) = self.workspace_change().or_else(|| self.selected_change()) else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&source.commit_id) {
            self.status = "cannot squash ROOT".to_string();
            return;
        }
        let destination = self
            .changes
            .iter()
            .find(|change| {
                change.commit_id == source.commit_id
                    && change.parent_commit_ids.len() == 1
                    && !is_root_commit(&change.parent_commit_ids[0])
            })
            .and_then(|change| self.change_by_commit(&change.parent_commit_ids[0]))
            .or_else(|| {
                self.changes
                    .iter()
                    .find(|change| change.commit_id == source.commit_id)
                    .and_then(|change| change.parent_commit_ids.first())
                    .and_then(|id| self.change_by_commit(id))
            });
        let Some(destination) = destination else {
            self.status = "squash destination parent not found".to_string();
            return;
        };
        self.set_pending_mutation(PendingMutation::SquashCurrent {
            source,
            destination,
        });
    }

    fn plan_abandon_selected(&mut self) {
        if self.block_historical_mutation("abandon") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot abandon ROOT".to_string();
            return;
        }
        let descendants = self.descendants_of(&change.commit_id);
        let bookmarks = self
            .changes
            .get(self.selected_change)
            .map(|change| change.bookmarks.clone())
            .unwrap_or_default();
        self.set_pending_mutation(PendingMutation::Abandon {
            change,
            descendants,
            bookmarks,
        });
    }

    fn begin_rebase_selected(&mut self) {
        if self.block_historical_mutation("rebase") {
            return;
        }
        let Some(source) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&source.commit_id) {
            self.status = "cannot rebase ROOT".to_string();
            return;
        }
        self.rebase_source = Some(source.clone());
        self.show_overlay(
            "Rebase source",
            vec![
                format!("source: {}", source.brief()),
                "Move the cursor to the destination change and press Enter.".to_string(),
                "Esc clears rebase selection.".to_string(),
            ],
        );
        self.pane = UiPane::Change;
        self.status = "select rebase destination".to_string();
    }

    fn finish_rebase_selection(&mut self) {
        if self.block_historical_mutation("rebase") {
            self.rebase_source = None;
            return;
        }
        let Some(source) = self.rebase_source.take() else {
            return;
        };
        let Some(destination) = self.selected_change() else {
            self.status = "no rebase destination selected".to_string();
            return;
        };
        if source.commit_id == destination.commit_id {
            self.status = "source and destination are the same change".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Rebase {
            source,
            destination,
        });
    }

    fn plan_restore_selected(&mut self) {
        if self.block_historical_mutation("restore") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot restore ROOT".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Restore { change });
    }

    fn plan_import_historical_file(&mut self) {
        if self.block_command("import historical file", CommandKind::ImportFromHistorical) {
            return;
        }
        let Some(source_op) = self.selected_operation_id() else {
            self.status = "no historical operation selected".to_string();
            return;
        };
        let Some(source) = self.selected_change() else {
            self.status = "no source change selected".to_string();
            return;
        };
        if is_root_commit(&source.commit_id) {
            self.status = "cannot import from ROOT".to_string();
            return;
        }
        let mut paths = self
            .selected_files
            .iter()
            .filter_map(|index| self.files.get(*index))
            .map(|file| file.path.clone())
            .filter(|path| !path.is_empty())
            .collect::<Vec<_>>();
        if paths.is_empty() {
            let path = self.selected_file_path().to_string();
            if !path.is_empty() {
                paths.push(path);
            }
        }
        if paths.is_empty() {
            self.status = "no file selected for import".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::ImportHistoricalFile {
            source_op,
            source,
            paths,
        });
    }

    fn begin_describe_selected(&mut self) {
        if self.block_historical_mutation("describe") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot describe ROOT".to_string();
            return;
        }
        self.describe_input = Some(DescribeInput {
            change_id: change.change_id.clone(),
            commit_id: change.commit_id.clone(),
            message: String::new(),
        });
        self.show_overlay(
            format!("Describe {}", short(&change.commit_id)),
            vec![
                "Type new description.".to_string(),
                "Enter: apply, Esc/N: cancel, Backspace: delete".to_string(),
                String::new(),
            ],
        );
        self.status = "describe input".to_string();
    }

    fn push_describe_char(&mut self, ch: char) {
        if let Some(input) = &mut self.describe_input {
            input.message.push(ch);
            self.render_describe_input();
        }
    }

    fn pop_describe_char(&mut self) {
        if let Some(input) = &mut self.describe_input {
            input.message.pop();
            self.render_describe_input();
        }
    }

    fn render_describe_input(&mut self) {
        let Some(input) = &self.describe_input else {
            return;
        };
        self.show_overlay(
            format!("Describe {}", short(&input.commit_id)),
            vec![
                "Type new description.".to_string(),
                "Enter: apply, Esc/N: cancel, Backspace: delete".to_string(),
                String::new(),
                input.message.clone(),
            ],
        );
    }

    fn cancel_describe_input(&mut self) {
        self.describe_input = None;
        self.clear_overlay();
        self.refresh_diff();
        self.status = "describe cancelled".to_string();
    }

    fn execute_describe_input(&mut self) {
        if self.block_historical_mutation("describe") {
            return;
        }
        let Some(input) = self.describe_input.take() else {
            return;
        };
        if input.message.trim().is_empty() {
            self.status = "description is empty; cancelled".to_string();
            return;
        }
        let args = vec![
            "describe".to_string(),
            input.commit_id.clone(),
            "-m".to_string(),
            input.message,
        ];
        match execute_mutation(args.clone()) {
            Ok(result) => {
                let success = result.exit_code == 0;
                if success {
                    self.refresh_after_mutation(Some(&input.change_id));
                }
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    Some(result.exit_code),
                    if success { "ok" } else { "failed" }.to_string(),
                );
                self.show_overlay("Mutation describe", command_result_lines(&result));
                self.status = if success {
                    format!("executed jj {}", args.join(" "))
                } else {
                    format!("describe failed: exit {}", result.exit_code)
                };
            }
            Err(error) => {
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    None,
                    "spawn failed".to_string(),
                );
                self.show_overlay(
                    "Mutation describe",
                    vec![format!("failed to execute jj {}: {error}", args.join(" "))],
                );
                self.status = "describe execution failed".to_string();
            }
        }
    }

    fn begin_commit_input(&mut self) {
        if self.block_historical_mutation("commit") {
            return;
        }
        self.commit_input = Some(CommitInput {
            message: String::new(),
        });
        self.show_overlay(
            "Commit",
            vec![
                "Type commit description.".to_string(),
                "Enter: confirm, Esc/N: cancel, Backspace: delete".to_string(),
                String::new(),
            ],
        );
        self.status = "commit input".to_string();
    }

    fn push_commit_char(&mut self, ch: char) {
        if let Some(input) = &mut self.commit_input {
            input.message.push(ch);
            self.render_commit_input();
        }
    }

    fn pop_commit_char(&mut self) {
        if let Some(input) = &mut self.commit_input {
            input.message.pop();
            self.render_commit_input();
        }
    }

    fn render_commit_input(&mut self) {
        let Some(input) = &self.commit_input else {
            return;
        };
        self.show_overlay(
            "Commit",
            vec![
                "Type commit description.".to_string(),
                "Enter: confirm, Esc/N: cancel, Backspace: delete".to_string(),
                String::new(),
                input.message.clone(),
            ],
        );
    }

    fn cancel_commit_input(&mut self) {
        self.commit_input = None;
        self.clear_overlay();
        self.refresh_diff();
        self.status = "commit cancelled".to_string();
    }

    fn confirm_commit_input(&mut self) {
        if self.block_historical_mutation("commit") {
            return;
        }
        let Some(input) = self.commit_input.take() else {
            return;
        };
        let message = input.message.trim().to_string();
        if message.is_empty() {
            self.status = "commit message is empty; cancelled".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::Commit { message });
    }

    fn begin_bookmark_input(&mut self) {
        if self.block_historical_mutation("bookmark") {
            return;
        }
        let Some(change) = self.selected_change() else {
            self.status = "no change selected".to_string();
            return;
        };
        if is_root_commit(&change.commit_id) {
            self.status = "cannot bookmark ROOT".to_string();
            return;
        }
        self.bookmark_input = Some(BookmarkInput {
            change,
            name: String::new(),
        });
        self.show_overlay(
            "Bookmark",
            vec![
                "Type local bookmark name.".to_string(),
                "Enter: confirm, Esc/N: cancel, Backspace: delete".to_string(),
                String::new(),
            ],
        );
        self.status = "bookmark input".to_string();
    }

    fn push_bookmark_char(&mut self, ch: char) {
        if let Some(input) = &mut self.bookmark_input {
            input.name.push(ch);
            self.render_bookmark_input();
        }
    }

    fn pop_bookmark_char(&mut self) {
        if let Some(input) = &mut self.bookmark_input {
            input.name.pop();
            self.render_bookmark_input();
        }
    }

    fn render_bookmark_input(&mut self) {
        let Some(input) = &self.bookmark_input else {
            return;
        };
        self.show_overlay(
            "Bookmark",
            vec![
                "Type local bookmark name.".to_string(),
                "Enter: confirm, Esc/N: cancel, Backspace: delete".to_string(),
                String::new(),
                input.name.clone(),
            ],
        );
    }

    fn cancel_bookmark_input(&mut self) {
        self.bookmark_input = None;
        self.clear_overlay();
        self.refresh_diff();
        self.status = "bookmark cancelled".to_string();
    }

    fn confirm_bookmark_input(&mut self) {
        if self.block_historical_mutation("bookmark") {
            return;
        }
        let Some(input) = self.bookmark_input.take() else {
            return;
        };
        let name = input.name.trim().to_string();
        if name.is_empty() {
            self.status = "bookmark name is empty; cancelled".to_string();
            return;
        }
        self.set_pending_mutation(PendingMutation::BookmarkSet {
            name,
            change: input.change,
        });
    }

    fn plan_push(&mut self) {
        if self.block_historical_mutation("push") {
            return;
        }
        let bookmark = self
            .changes
            .get(self.selected_change)
            .and_then(TuiChange::local_bookmark);
        self.set_pending_mutation(PendingMutation::Push { bookmark });
    }

    fn set_pending_mutation(&mut self, mutation: PendingMutation) {
        if self.block_command(mutation.name(), mutation.command_kind()) {
            return;
        }
        let mut lines = self.operation_context_lines();
        lines.push(String::new());
        lines.extend(mutation.preview_lines());
        self.show_overlay(format!("Confirm {}", mutation.name()), lines);
        self.status = "confirm mutation with y".to_string();
        self.pending_mutation = Some(mutation);
    }

    fn cancel_pending_mutation(&mut self) {
        self.pending_mutation = None;
        self.clear_overlay();
        self.refresh_diff();
        self.status = "mutation cancelled".to_string();
    }

    fn execute_pending_mutation(&mut self) {
        let Some(kind) = self
            .pending_mutation
            .as_ref()
            .map(PendingMutation::command_kind)
        else {
            self.status = "no pending mutation".to_string();
            return;
        };
        if self.block_command("execute mutation", kind) {
            return;
        }
        let Some(mutation) = self.pending_mutation.take() else {
            self.status = "no pending mutation".to_string();
            return;
        };
        let preferred_change_id = mutation.preferred_change_id().map(str::to_string);
        let args = mutation.command_args();
        let result = execute_mutation(args.clone());
        match result {
            Ok(result) => {
                let success = result.exit_code == 0;
                if success {
                    self.refresh_after_mutation(preferred_change_id.as_deref());
                }
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    Some(result.exit_code),
                    if success { "ok" } else { "failed" }.to_string(),
                );
                self.show_overlay(
                    format!("Mutation {}", mutation.name()),
                    command_result_lines(&result),
                );
                self.status = if success {
                    format!("executed jj {}", args.join(" "))
                } else {
                    format!("mutation failed: exit {}", result.exit_code)
                };
            }
            Err(error) => {
                self.push_command_log(
                    format!("jj {}", args.join(" ")),
                    None,
                    "spawn failed".to_string(),
                );
                self.show_overlay(
                    format!("Mutation {}", mutation.name()),
                    vec![format!("failed to execute jj {}: {error}", args.join(" "))],
                );
                self.status = "mutation execution failed".to_string();
            }
        }
    }

    fn selected_editor_file(&self) -> Result<String, String> {
        let path = self.selected_file_path();
        if path.is_empty() {
            return Err("no file selected for editor".to_string());
        }
        let absolute = std::env::current_dir()
            .map_err(|error| format!("failed to resolve repo directory: {error}"))?
            .join(path);
        if !absolute.exists() {
            return Err(format!(
                "selected file is not present in working tree: {}",
                absolute.display()
            ));
        }
        absolute
            .canonicalize()
            .map(|path| path.to_string_lossy().into_owned())
            .map_err(|error| format!("failed to resolve file path: {error}"))
    }
}

fn run_tui_or_exit() {
    if let Err(error) = run_tui() {
        eprintln!("tui failed: {error}");
        std::process::exit(1);
    }
}

fn run_tui() -> io::Result<()> {
    enable_raw_mode()?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)?;
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout))?;
    let result = tui_loop(&mut terminal);
    disable_raw_mode()?;
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.show_cursor()?;
    result
}

fn tui_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> io::Result<()> {
    let mut app = TuiApp::new();
    let launcher = ConfiguredLauncher {
        settings: load_editor_settings(),
    };

    loop {
        terminal.draw(|frame| {
            let operation_labels = app.operation_labels();
            let change_labels = app.change_labels();
            let file_labels = app.file_labels();
            let operation_title = app.operation_title();
            let change_title = app.change_title();
            let files_title = app.files_title();
            let areas = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Min(3),
                    Constraint::Length(3),
                    Constraint::Length(3),
                ])
                .split(frame.size());
            let body = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(28),
                    Constraint::Percentage(28),
                    Constraint::Percentage(44),
                ])
                .split(areas[0]);
            let middle = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Percentage(55), Constraint::Percentage(45)])
                .split(body[1]);
            app.operation_visible_rows = body[0].height.saturating_sub(2).max(1) as usize;
            app.change_visible_rows = middle[0].height.saturating_sub(2).max(1) as usize;
            app.file_visible_rows = middle[1].height.saturating_sub(2).max(1) as usize;
            app.keep_selected_operation_visible();
            app.keep_selected_change_visible();
            app.keep_selected_file_visible();

            let render_operation = |frame: &mut ratatui::Frame<'_>, area| {
                frame.render_widget(
                    selectable_list(
                        &operation_title,
                        &visible_from(&operation_labels, app.operation_scroll),
                        app.selected_operation,
                        app.operation_scroll,
                        app.pane == UiPane::Operation || app.pane == UiPane::Repository,
                        Color::Cyan,
                    ),
                    area,
                );
            };
            let render_change = |frame: &mut ratatui::Frame<'_>, area| {
                frame.render_widget(
                    selectable_list(
                        &change_title,
                        &visible_from(&change_labels, app.change_scroll),
                        app.selected_change,
                        app.change_scroll,
                        app.pane == UiPane::Change,
                        Color::Green,
                    ),
                    area,
                );
            };
            let render_files = |frame: &mut ratatui::Frame<'_>, area| {
                frame.render_widget(
                    selectable_list(
                        &files_title,
                        &visible_from(&file_labels, app.file_scroll),
                        app.selected_file,
                        app.file_scroll,
                        app.pane == UiPane::Files,
                        Color::Magenta,
                    ),
                    area,
                );
            };
            let render_diff = |frame: &mut ratatui::Frame<'_>, area| {
                frame.render_widget(
                    Paragraph::new(styled_lines(&app.detail))
                        .block(focused_block(
                            app.detail_title.as_str(),
                            app.pane == UiPane::Diff,
                            Color::Yellow,
                        ))
                        .scroll((app.detail_scroll, 0))
                        .wrap(Wrap { trim: false }),
                    area,
                );
            };

            match app.maximized_pane {
                Some(UiPane::Operation | UiPane::Repository) => render_operation(frame, areas[0]),
                Some(UiPane::Change) => render_change(frame, areas[0]),
                Some(UiPane::Files) => render_files(frame, areas[0]),
                Some(UiPane::Diff) => render_diff(frame, areas[0]),
                None => {
                    render_operation(frame, body[0]);
                    render_change(frame, middle[0]);
                    render_files(frame, middle[1]);
                    render_diff(frame, body[2]);
                }
            }
            frame.render_widget(
                Paragraph::new(status_line(&app)).block(Block::default().borders(Borders::ALL)),
                areas[1],
            );
            frame.render_widget(
                Paragraph::new(bottom_key_hint(&app)).block(Block::default().borders(Borders::ALL)),
                areas[2],
            );
            if let Some(overlay) = &app.overlay {
                let area = centered_rect(35, 50, frame.size());
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(styled_lines(&overlay.lines))
                        .block(focused_block(overlay.title.as_str(), true, Color::Yellow))
                        .scroll((overlay.scroll, 0))
                        .wrap(Wrap { trim: false }),
                    area,
                );
                if let Some(cursor) = app.input_cursor() {
                    frame.set_cursor(
                        area.x.saturating_add(1).saturating_add(cursor.column),
                        area.y.saturating_add(1).saturating_add(cursor.line),
                    );
                }
            }
        })?;

        let event = event::read()?;
        let Event::Key(key) = event else {
            if let Event::Mouse(mouse) = event {
                let size = terminal.size()?;
                match mouse.kind {
                    MouseEventKind::Down(_) => {
                        app.focus_at(
                            mouse.column,
                            mouse.row,
                            size.width,
                            size.height,
                            mouse.modifiers,
                        );
                    }
                    MouseEventKind::ScrollUp => {
                        app.scroll_at(mouse.column, mouse.row, size.width, size.height, -3);
                    }
                    MouseEventKind::ScrollDown => {
                        app.scroll_at(mouse.column, mouse.row, size.width, size.height, 3);
                    }
                    _ => {}
                }
            }
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        if app.describe_input.is_some() {
            match key.code {
                KeyCode::Enter => app.execute_describe_input(),
                KeyCode::Esc | KeyCode::Char('N') => app.cancel_describe_input(),
                KeyCode::Backspace => app.pop_describe_char(),
                KeyCode::Char(ch) => app.push_describe_char(ch),
                _ => app.status = "describe input: type, Enter apply, Esc/N cancel".to_string(),
            }
            continue;
        }
        if app.commit_input.is_some() {
            match key.code {
                KeyCode::Enter => app.confirm_commit_input(),
                KeyCode::Esc | KeyCode::Char('N') => app.cancel_commit_input(),
                KeyCode::Backspace => app.pop_commit_char(),
                KeyCode::Char(ch) => app.push_commit_char(ch),
                _ => app.status = "commit input: type, Enter confirm, Esc/N cancel".to_string(),
            }
            continue;
        }
        if app.bookmark_input.is_some() {
            match key.code {
                KeyCode::Enter => app.confirm_bookmark_input(),
                KeyCode::Esc | KeyCode::Char('N') => app.cancel_bookmark_input(),
                KeyCode::Backspace => app.pop_bookmark_char(),
                KeyCode::Char(ch) => app.push_bookmark_char(ch),
                _ => app.status = "bookmark input: type, Enter confirm, Esc/N cancel".to_string(),
            }
            continue;
        }
        if app.pending_mutation.is_some() {
            match key.code {
                KeyCode::Char('y') | KeyCode::Char('Y') => app.execute_pending_mutation(),
                KeyCode::Esc | KeyCode::Char('N') => app.cancel_pending_mutation(),
                _ => {
                    app.status = "pending mutation: press y to execute, Esc/N to cancel".to_string()
                }
            }
            continue;
        }
        let action = match key.code {
            KeyCode::Esc if app.rebase_source.is_some() => {
                app.rebase_source = None;
                app.clear_overlay();
                app.refresh_diff();
                app.status = "rebase selection cancelled".to_string();
                continue;
            }
            KeyCode::Esc if app.overlay.is_some() => {
                app.clear_overlay();
                app.set_browse_status("overlay closed");
                continue;
            }
            KeyCode::Enter
                if app.overlay.is_some() && key.modifiers.contains(KeyModifiers::SHIFT) =>
            {
                app.scroll_overlay_page(-12);
                continue;
            }
            KeyCode::Enter if app.overlay.is_some() => {
                app.scroll_overlay_page(12);
                continue;
            }
            KeyCode::Esc if app.mode == UiMode::Surgery => {
                app.exit_surgery_mode();
                continue;
            }
            _ if app.overlay.is_some() => {
                app.status = "overlay open; Esc closes it".to_string();
                continue;
            }
            KeyCode::Char('1') => {
                app.pane = UiPane::Operation;
                app.set_browse_status("pane: Operation");
                continue;
            }
            KeyCode::Char('2') => {
                app.pane = UiPane::Change;
                app.set_browse_status("pane: Change");
                continue;
            }
            KeyCode::Char('3') => {
                app.pane = UiPane::Files;
                app.set_browse_status("pane: Files");
                continue;
            }
            KeyCode::Char('4') => {
                app.pane = UiPane::Diff;
                app.set_browse_status("pane: Diff");
                continue;
            }
            KeyCode::Char('o') => {
                app.status = "current-terminal editor disabled; use O".to_string();
                continue;
            }
            KeyCode::Char('O') => {
                app.status = match app.selected_editor_file() {
                    Ok(file) => match launcher.open_editor(OpenRequest {
                        file: file.clone(),
                        placement: launcher.settings.placement,
                    }) {
                        Ok(()) => {
                            app.push_command_log(
                                format!("open editor {file}"),
                                Some(0),
                                "ok".to_string(),
                            );
                            "editor launched".to_string()
                        }
                        Err(error) => {
                            app.push_command_log(
                                format!("open editor {file}"),
                                None,
                                "failed".to_string(),
                            );
                            error
                        }
                    },
                    Err(error) => error,
                };
                continue;
            }
            KeyCode::Char('U') => {
                app.plan_undo();
                continue;
            }
            KeyCode::Char('e') => {
                app.plan_edit_selected();
                continue;
            }
            KeyCode::Char('n') => {
                app.plan_new_child_selected();
                continue;
            }
            KeyCode::Char('d') => {
                app.begin_describe_selected();
                continue;
            }
            KeyCode::Char('c') => {
                app.begin_commit_input();
                continue;
            }
            KeyCode::Char('B') => {
                app.begin_bookmark_input();
                continue;
            }
            KeyCode::Char('p') => {
                app.plan_push();
                continue;
            }
            KeyCode::Char('S') => {
                app.enter_surgery_mode();
                continue;
            }
            KeyCode::Char('s') => {
                app.plan_squash_current();
                continue;
            }
            KeyCode::Char('a') => {
                if app.mode == UiMode::Surgery {
                    app.preview_surgery_abandon_selected_operation();
                } else {
                    app.plan_abandon_selected();
                }
                continue;
            }
            KeyCode::Char('b') => {
                app.begin_rebase_selected();
                continue;
            }
            KeyCode::Char('Z') => {
                app.plan_restore_selected();
                continue;
            }
            KeyCode::Char('z') => {
                app.toggle_maximize_pane();
                continue;
            }
            KeyCode::Char('I') => {
                app.plan_import_historical_file();
                continue;
            }
            KeyCode::Char('?') => {
                app.show_help();
                continue;
            }
            KeyCode::Char('L') => {
                app.show_command_log();
                continue;
            }
            KeyCode::Char(' ') => {
                app.toggle_pin_comparison();
                continue;
            }
            KeyCode::Char('h') => {
                app.move_graph_relative(true);
                continue;
            }
            KeyCode::Char('l') => {
                app.move_graph_relative(false);
                continue;
            }
            KeyCode::Up if is_command_modifier(key.modifiers) => {
                app.jump_selection(false);
                continue;
            }
            KeyCode::Down if is_command_modifier(key.modifiers) => {
                app.jump_selection(true);
                continue;
            }
            KeyCode::Up
                if key.modifiers.contains(KeyModifiers::SHIFT) && app.pane == UiPane::Files =>
            {
                app.extend_file_selection(-1);
                continue;
            }
            KeyCode::Down
                if key.modifiers.contains(KeyModifiers::SHIFT) && app.pane == UiPane::Files =>
            {
                app.extend_file_selection(1);
                continue;
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                if !app.scroll_overlay_page(-12) {
                    app.scroll_detail_page(-12);
                }
                continue;
            }
            KeyCode::Enter if app.rebase_source.is_some() => {
                app.finish_rebase_selection();
                continue;
            }
            KeyCode::Enter => {
                app.open_focused();
                continue;
            }
            KeyCode::BackTab => {
                app.focus_previous();
                continue;
            }
            KeyCode::Down => KeyAction::Char('j'),
            KeyCode::Up => KeyAction::Char('k'),
            KeyCode::Char('r') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                KeyAction::Ctrl('r')
            }
            KeyCode::Char(ch) => KeyAction::Char(ch),
            KeyCode::Tab => KeyAction::Tab,
            _ => KeyAction::Char('\0'),
        };

        match handle_key_input(action, app.pane) {
            UiCommand::SwitchPane { to, .. } => {
                app.pane = if to == UiPane::Repository {
                    UiPane::Operation
                } else {
                    to
                };
                app.set_browse_status(format!("pane: {to:?}"));
            }
            UiCommand::RefreshReadOnly => {
                app.refresh_all();
            }
            UiCommand::PreviewUndo => {
                app.set_preview("Preview undo", &["preview", "undo"]);
            }
            UiCommand::PreviewRedo => {
                app.set_preview("Preview redo", &["preview", "redo"]);
            }
            UiCommand::PreviewSnapshot => {
                app.set_preview("Preview snapshot", &["snapshot"]);
            }
            UiCommand::Quit => return Ok(()),
            UiCommand::MoveSelection { delta } => app.move_selection(delta),
            UiCommand::Noop => {}
        }
    }
}

fn bottom_key_hint(app: &TuiApp) -> Line<'static> {
    Line::from(vec![
        Span::styled("Keys ", Style::default().fg(Color::Green)),
        Span::raw(context_key_hint(app)),
    ])
}

fn context_key_hint(app: &TuiApp) -> &'static str {
    if app.overlay.is_some() {
        return "Esc:close Enter/Shift+Enter:scroll ";
    }
    match (app.mode, app.pane) {
        (UiMode::Surgery, UiPane::Operation | UiPane::Repository) => {
            "Tab:Pane j/k:Move h/l:Graph a:preview abandon Esc:Browse z:max ? "
        }
        (_, UiPane::Operation | UiPane::Repository) => {
            "Tab:Pane j/k:Move h/l:Graph Enter:detail S:Surgery z:max ? "
        }
        (_, UiPane::Change) => {
            "Tab:Pane j/k:Move h/l:Graph Space:Pin e:edit n:new d:describe a:abandon s:squash b:rebase Z:restore z:max ? "
        }
        (_, UiPane::Files) => {
            "Tab:Pane j/k:Move Shift+↑↓:range Cmd+click:toggle Enter:diff O:open I:import z:max ? "
        }
        (_, UiPane::Diff) => "Tab:Pane j/k:Scroll Enter:page Shift+Enter:page up z:max L:log ? ",
    }
}

fn pane_mode_help() -> Vec<String> {
    vec![
        "Pane x Mode".to_string(),
        String::new(),
        "Pane        Inspect / Current                         Majutsu / Historical                  Shujutsu / Surgery".to_string(),
        "Operation   h/l graph, Enter detail, S surgery         read-only detail, import via Files    a preview op abandon, Esc browse".to_string(),
        "Change      e n d a s b Z, Space pin                   read-only; mutation blocked           no change mutation".to_string(),
        "Files       Enter diff, O open, range select           I import historical file              no file mutation".to_string(),
        "Diff        j/k scroll, Enter page, Shift+Enter up     same                                same".to_string(),
        String::new(),
        "Global      Tab/S-Tab pane, 1-4 focus, z maximize, L command log, ? help, q quit".to_string(),
    ]
}

fn status_line(app: &TuiApp) -> Line<'_> {
    let mut spans = app.context_spans();
    spans.push(Span::raw(" | "));
    spans.push(Span::styled(
        app.status.as_str(),
        Style::default().fg(app.status_color()),
    ));
    Line::from(spans)
}

fn command_result_lines(result: &vijutsu::cli::CommandResult) -> Vec<String> {
    let mut lines = vec![
        format!("command: {}", result.command_display),
        format!("exit: {}", result.exit_code),
    ];
    if !result.stdout.trim().is_empty() {
        lines.push("stdout:".to_string());
        lines.extend(result.stdout.trim().lines().map(str::to_string));
    }
    if !result.stderr.trim().is_empty() {
        lines.push("stderr:".to_string());
        lines.extend(result.stderr.trim().lines().map(str::to_string));
    }
    lines
}

fn load_operations() -> Result<(String, String, Vec<TuiOperation>), String> {
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

fn annotate_operation_states(version: &str, operations: &mut [TuiOperation]) {
    let fingerprints = operations
        .iter()
        .map(|operation| operation_state_fingerprint(version, &operation.id))
        .collect::<Vec<_>>();
    let op_ids = operations
        .iter()
        .map(|operation| operation.id.clone())
        .collect::<Vec<_>>();

    for (operation, suffix) in operations
        .iter_mut()
        .zip(state_suffixes(&op_ids, &fingerprints))
    {
        if !suffix.is_empty() {
            operation.label = label_with_state_suffix(&operation.label, &suffix);
        }
    }
}

fn label_with_state_suffix(label: &str, suffix: &str) -> String {
    let suffix = suffix.trim();
    if suffix.is_empty() {
        return label.to_string();
    }
    match label.split_once(' ') {
        Some((op_id, rest)) => format!("{op_id} {suffix} {rest}"),
        None => format!("{label} {suffix}"),
    }
}

fn operation_state_fingerprint(version: &str, op_id: &str) -> Option<StateFingerprint> {
    let parse = fetch_change_records(version, op_id, "all()").ok()?;
    fail_tui_parse("state fingerprint change graph", &parse).ok()?;
    Some(state_fingerprint(&parse.records))
}

fn load_changes(version: &str, op_id: &str) -> Result<Vec<TuiChange>, String> {
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

fn fail_tui_parse<T>(context: &str, result: &ParseResult<T>) -> Result<(), String> {
    parse_diagnostic(context, &result.errors, &result.raw_stdout)
        .map(Err)
        .unwrap_or(Ok(()))
}

fn non_empty_or(lines: Vec<String>, fallback: &str) -> Vec<String> {
    if lines.is_empty() {
        vec![fallback.to_string()]
    } else {
        lines
    }
}

fn selected_line(lines: &[String], index: usize) -> String {
    lines
        .get(index)
        .cloned()
        .unwrap_or_else(|| "(nothing selected)".to_string())
}

fn bookmark_label_suffix(bookmarks: &[String]) -> String {
    if bookmarks.is_empty() {
        String::new()
    } else {
        format!(
            " [{}]",
            bookmarks
                .iter()
                .map(|bookmark| bookmark_display(bookmark))
                .collect::<Vec<_>>()
                .join(" ")
        )
    }
}

fn bookmark_display(bookmark: &str) -> String {
    if bookmark.contains('@') {
        format!("{bookmark}:remote")
    } else {
        format!("{bookmark}:local")
    }
}

fn operation_semantic_suffix(description: &str) -> Option<String> {
    let (label, rest) = [
        ("undo: restore to operation ", "undo target"),
        ("restore to operation ", "state-from"),
        ("restore operation ", "state-from"),
        ("revert operation ", "target"),
        ("undo operation ", "target"),
    ]
    .into_iter()
    .find_map(|(prefix, label)| description.strip_prefix(prefix).map(|rest| (label, rest)))?;
    let target = rest.split_whitespace().next()?;
    Some(format!(" [{label}: {}]", short(target)))
}

fn non_empty_join(values: &[String], fallback: &str) -> String {
    if values.is_empty() {
        fallback.to_string()
    } else {
        values.join(", ")
    }
}

fn clamp_index(index: usize, len: usize) -> usize {
    if len == 0 { 0 } else { index.min(len - 1) }
}

fn move_index(index: usize, len: usize, delta: isize) -> usize {
    if len == 0 {
        return 0;
    }
    index
        .saturating_add_signed(delta)
        .min(len.saturating_sub(1))
}

fn keep_selected_visible(selected: usize, scroll: &mut usize, visible_rows: usize) {
    let visible_rows = visible_rows.max(1);
    if selected < *scroll {
        *scroll = selected;
    } else if selected >= *scroll + visible_rows {
        *scroll = selected + 1 - visible_rows;
    }
}

fn is_command_modifier(modifiers: KeyModifiers) -> bool {
    modifiers.intersects(KeyModifiers::SUPER | KeyModifiers::META)
}

fn pane_at(column: u16, row: u16, width: u16, height: u16) -> Option<UiPane> {
    let body_height = height.saturating_sub(6);
    if row >= body_height {
        return None;
    }
    if column < width.saturating_mul(28) / 100 {
        Some(UiPane::Operation)
    } else if column < width.saturating_mul(56) / 100 {
        if row < body_height.saturating_mul(55) / 100 {
            Some(UiPane::Change)
        } else {
            Some(UiPane::Files)
        }
    } else {
        Some(UiPane::Diff)
    }
}

fn centered_rect(percent_x: u16, percent_y: u16, area: Rect) -> Rect {
    let vertical = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Percentage((100 - percent_y) / 2),
            Constraint::Percentage(percent_y),
            Constraint::Percentage((100 - percent_y) / 2),
        ])
        .split(area);
    Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Percentage((100 - percent_x) / 2),
            Constraint::Percentage(percent_x),
            Constraint::Percentage((100 - percent_x) / 2),
        ])
        .split(vertical[1])[1]
}

fn is_root_commit(commit_id: &str) -> bool {
    commit_id.chars().all(|ch| ch == '0')
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
}

fn run_self(args: &[&str]) -> String {
    let exe = match std::env::current_exe() {
        Ok(exe) => exe,
        Err(error) => return format!("failed to find current executable: {error}"),
    };
    match Command::new(exe).args(args).output() {
        Ok(output) => {
            let mut text = String::new();
            text.push_str(&String::from_utf8_lossy(&output.stdout));
            text.push_str(&String::from_utf8_lossy(&output.stderr));
            if text.trim().is_empty() {
                format!("command exited with {}", output.status)
            } else {
                text
            }
        }
        Err(error) => format!("failed to run vijutsu {}: {error}", args.join(" ")),
    }
}

fn poll(args: Vec<String>) {
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

fn preview(args: Vec<String>) {
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

fn mutate(args: Vec<String>) {
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

fn snapshot(args: Vec<String>) {
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

fn preview_restore(target_op: &str) {
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

fn preview_undo() {
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

fn preview_revert(target_op: &str) {
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

fn preview_abandon(target_ops: &[String]) {
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

fn preview_redo() {
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

fn print_capabilities() {
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

fn print_command_result(result: &vijutsu::cli::CommandResult) {
    println!("command: {}", result.command_display);
    println!("exit: {}", result.exit_code);
    if !result.stdout.trim().is_empty() {
        println!("stdout:\n{}", result.stdout.trim());
    }
    if !result.stderr.trim().is_empty() {
        eprintln!("stderr:\n{}", result.stderr.trim());
    }
}

fn require_mutation_protocol_capabilities(caps: &JjCapabilities) -> Result<(), String> {
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

fn jj_version_string() -> String {
    let result = jj_version().unwrap_or_else(|error| fail_io("run jj version", error));
    if result.exit_code != 0 {
        eprintln!("{}", result.stderr.trim());
        std::process::exit(result.exit_code);
    }
    result.stdout.trim().to_string()
}

fn current_operation_id(version: &str) -> String {
    current_operation(version).operation_id
}

fn current_operation(version: &str) -> vijutsu::core::OperationNode {
    let graph = fetch_operation_graph(version, "@", 1, "current operation");
    graph
        .nodes
        .into_iter()
        .next()
        .unwrap_or_else(|| fail_message("current operation not found"))
}

fn fetch_change_graph(version: &str, at_op: &str, revset: &str) -> vijutsu::core::ChangeGraph {
    let parse = fetch_change_records(version, at_op, revset)
        .unwrap_or_else(|error| fail_io("fetch preview change graph", error));
    fail_parse_result("preview change graph", &parse);
    change_graph_from_parse(parse)
}

fn fetch_operation_graph(
    version: &str,
    at_op: &str,
    limit: usize,
    context: &str,
) -> vijutsu::core::OperationGraph {
    let parse = fetch_operation_records(version, at_op, limit)
        .unwrap_or_else(|error| fail_io(context, error));
    fail_parse_result(context, &parse);
    operation_graph_from_parse(parse)
}

fn resolve_operation_prefix(requested: &str, known_ids: &[&str]) -> Result<String, String> {
    let matches = known_ids
        .iter()
        .filter(|id| id.starts_with(requested))
        .collect::<Vec<_>>();
    match matches.len() {
        0 => Ok(requested.to_string()),
        1 => Ok((*matches[0]).to_string()),
        _ => Err(format!("ambiguous operation prefix: {requested}")),
    }
}

fn missing_arg(message: &str) -> ! {
    eprintln!("missing argument: {message}");
    std::process::exit(2);
}

fn fail_message(message: &str) -> ! {
    eprintln!("{message}");
    std::process::exit(1);
}

fn fail_parse_result<T>(context: &str, result: &ParseResult<T>) {
    fail_parse(context, &result.errors, &result.raw_stdout);
}

fn fail_parse(context: &str, errors: &[ParseError], raw_stdout: &str) {
    let Some(diagnostic) = parse_diagnostic(context, errors, raw_stdout) else {
        return;
    };

    eprint!("{diagnostic}");
    std::process::exit(1);
}

fn parse_diagnostic(context: &str, errors: &[ParseError], raw_stdout: &str) -> Option<String> {
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

fn fail_io(context: &str, error: std::io::Error) -> ! {
    eprintln!("failed to {context}: {error}");
    std::process::exit(1);
}

struct InspectOptions {
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

struct PollOptions {
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
    fn operation_prefix_resolves_known_unique_id() {
        assert_eq!(
            resolve_operation_prefix("abc", &["abcdef", "123456"]).unwrap(),
            "abcdef"
        );
        assert_eq!(
            resolve_operation_prefix("missing", &["abcdef"]).unwrap(),
            "missing"
        );
        assert!(resolve_operation_prefix("a", &["abcdef", "axyz"]).is_err());
    }

    #[test]
    fn operation_state_suffix_is_inserted_after_operation_id() {
        assert_eq!(
            label_with_state_suffix("abc12345 undo def67890 [state ≈ xyz]", " S1 (= oldop)"),
            "abc12345 S1 (= oldop) undo def67890 [state ≈ xyz]"
        );
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

    #[test]
    fn operation_semantic_suffix_shows_references_without_graph_edges() {
        assert_eq!(
            operation_semantic_suffix(
                "undo: restore to operation abcdef1234567890 trailing ignored"
            )
            .as_deref(),
            Some(" [undo target: abcdef12]")
        );
        assert_eq!(
            operation_semantic_suffix("restore to operation fedcba9876543210").as_deref(),
            Some(" [state-from: fedcba98]")
        );
        assert_eq!(
            operation_semantic_suffix("revert operation 1234567890abcdef").as_deref(),
            Some(" [target: 12345678]")
        );
        assert!(operation_semantic_suffix("abandon commit abcdef").is_none());
    }

    #[test]
    fn bookmark_label_suffix_marks_local_and_remote() {
        assert_eq!(
            bookmark_label_suffix(&["feature".to_string(), "main@origin".to_string()]),
            " [feature:local main@origin:remote]"
        );
    }

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
