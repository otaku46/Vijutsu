use std::fs;
use std::io;
use std::path::PathBuf;
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
use ratatui::layout::{Constraint, Direction, Layout};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, List, ListItem, Paragraph, Wrap};

use vijutsu::cli::{
    ParseError, ParseResult, detect_capabilities, execute_snapshot_refresh, fetch_change_records,
    fetch_diff, fetch_diff_for_path, fetch_file_summary, fetch_operation_records, jj_version,
};
use vijutsu::core::{JjCapabilities, ObservationFreshness};
use vijutsu::mutation::{
    RestorePreview, RevertPreview, UndoPreview, abandon_preview, execute_mutation, redo_preview,
    restore_warnings, snapshot_refresh_result, snapshot_refresh_warnings, undo_warnings,
};
use vijutsu::observation::{
    SelectionState, build_observation, change_graph_from_parse, compute_operation_delta,
    operation_graph_from_parse, resolve_change_selection, validate_head_consistency,
};
use vijutsu::tui::{
    ChangeDetails, DeltaDetails, KeyAction, UiCommand, UiPane, handle_key_input,
    preview_panel_for_abandon, preview_panel_for_redo, preview_panel_for_restore,
    preview_panel_for_revert, preview_panel_for_snapshot_refresh, preview_panel_for_undo,
    render_keys, render_multiple_heads, render_observation, render_preview_panel,
};
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

fn print_keys() {
    print!("{}", render_keys());
}

struct TuiApp {
    pane: UiPane,
    operations: Vec<TuiOperation>,
    changes: Vec<TuiChange>,
    files: Vec<TuiFile>,
    detail_title: String,
    detail: Vec<String>,
    selected_operation: usize,
    selected_change: usize,
    selected_file: usize,
    file_scroll: usize,
    file_visible_rows: usize,
    detail_scroll: u16,
    version: String,
    current_head: String,
    observed_at: u64,
    status: String,
}

struct TuiOperation {
    id: String,
    label: String,
    is_current: bool,
}

struct TuiChange {
    change_id: String,
    commit_id: String,
    label: String,
}

struct TuiFile {
    path: String,
    label: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OpenPlacement {
    CurrentTerminal,
    NewPane,
    NewTab,
    NewWindow,
}

struct OpenRequest {
    file: String,
    placement: OpenPlacement,
}

trait TerminalLauncher {
    fn open_editor(&self, request: OpenRequest) -> Result<(), String>;
}

struct EditorSettings {
    command: Vec<String>,
    placement: OpenPlacement,
    launcher: String,
}

struct ConfiguredLauncher {
    settings: EditorSettings,
}

impl TuiApp {
    fn new() -> Self {
        let mut app = Self {
            pane: UiPane::Operation,
            operations: Vec::new(),
            changes: Vec::new(),
            files: Vec::new(),
            detail_title: "4 Detail".to_string(),
            detail: vec![
                "Enter: show focused item here".to_string(),
                "u: undo preview  Ctrl-r: redo preview  R: snapshot preview".to_string(),
            ],
            selected_operation: 0,
            selected_change: 0,
            selected_file: 0,
            file_scroll: 0,
            file_visible_rows: 1,
            detail_scroll: 0,
            version: String::new(),
            current_head: "?".to_string(),
            observed_at: 0,
            status: "read-only; mutations are not executed in TUI".to_string(),
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
                self.status = format!("gen {} sync; stale async results: none", self.observed_at);
            }
            Err(error) => self.status = error,
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

    fn selected_commit_id(&self) -> Option<String> {
        self.changes
            .get(self.selected_change)
            .map(|change| change.commit_id.clone())
    }

    fn selected_file_path(&self) -> &str {
        self.files
            .get(self.selected_file)
            .map(|file| file.path.as_str())
            .unwrap_or("")
    }

    fn context(&self) -> String {
        let op = self
            .selected_operation_id()
            .map(|id| short(&id).to_string())
            .unwrap_or_else(|| "?".to_string());
        let mode = self
            .operations
            .get(self.selected_operation)
            .map(|operation| {
                if operation.is_current {
                    "current"
                } else {
                    "historical"
                }
            })
            .unwrap_or("unknown");
        format!(
            "op {op} | current head {} | {mode} | snapshot only | fs unchanged | gen {}",
            short(&self.current_head),
            self.observed_at
        )
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
        self.operations
            .iter()
            .map(|operation| operation.label.clone())
            .collect()
    }

    fn change_labels(&self) -> Vec<String> {
        self.changes
            .iter()
            .map(|change| change.label.clone())
            .collect()
    }

    fn file_labels(&self) -> Vec<String> {
        self.files.iter().map(|file| file.label.clone()).collect()
    }

    fn focus_previous(&mut self) {
        self.pane = match self.pane {
            UiPane::Operation | UiPane::Repository => UiPane::Diff,
            UiPane::Change => UiPane::Operation,
            UiPane::Files => UiPane::Change,
            UiPane::Diff => UiPane::Files,
        };
        self.status = format!("pane: {:?}", self.pane);
    }

    fn focus_at(&mut self, column: u16, row: u16, width: u16, height: u16) {
        let body_height = height.saturating_sub(3);
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        let item_row = row.saturating_sub(1) as usize;
        let op_height = body_height.saturating_sub(8);
        self.pane = if row >= body_height {
            self.pane
        } else if column < width.saturating_mul(28) / 100 {
            if row < op_height {
                self.selected_operation = clamp_index(item_row, self.operations.len());
            }
            UiPane::Operation
        } else if column < width.saturating_mul(56) / 100 {
            if row < body_height.saturating_mul(55) / 100 {
                self.selected_change = clamp_index(item_row, self.changes.len());
                UiPane::Change
            } else {
                let files_top = body_height.saturating_mul(55) / 100;
                let row_in_files = row.saturating_sub(files_top).saturating_sub(1) as usize;
                self.selected_file = clamp_index(self.file_scroll + row_in_files, self.files.len());
                UiPane::Files
            }
        } else {
            UiPane::Diff
        };
        self.sync_after_selection(old_operation, old_change, old_file);
        self.status = format!("pane: {:?}", self.pane);
    }

    fn sync_after_selection(&mut self, old_operation: usize, old_change: usize, old_file: usize) {
        if self.selected_operation != old_operation {
            self.refresh_changes();
        } else if self.selected_change != old_change {
            self.refresh_files();
        } else if self.selected_file != old_file {
            self.refresh_diff();
        } else {
            return;
        }
        self.detail_scroll = 0;
    }

    fn move_selection(&mut self, delta: isize) {
        let old_operation = self.selected_operation;
        let old_change = self.selected_change;
        let old_file = self.selected_file;
        match self.pane {
            UiPane::Operation | UiPane::Repository => {
                self.selected_operation =
                    move_index(self.selected_operation, self.operations.len(), delta);
            }
            UiPane::Change => {
                self.selected_change = move_index(self.selected_change, self.changes.len(), delta);
            }
            UiPane::Files => {
                self.selected_file = move_index(self.selected_file, self.files.len(), delta);
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

    fn keep_selected_file_visible(&mut self) {
        let visible_rows = self.file_visible_rows.max(1);
        if self.selected_file < self.file_scroll {
            self.file_scroll = self.selected_file;
        } else if self.selected_file >= self.file_scroll + visible_rows {
            self.file_scroll = self.selected_file + 1 - visible_rows;
        }
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
        let (title, line) = match self.pane {
            UiPane::Operation | UiPane::Repository => (
                "4 Operation",
                selected_line(&self.operation_labels(), self.selected_operation),
            ),
            UiPane::Change => (
                "4 Change",
                selected_line(&self.change_labels(), self.selected_change),
            ),
            UiPane::Files => {
                self.refresh_diff();
                self.pane = UiPane::Diff;
                self.status = "opened file diff".to_string();
                return;
            }
            UiPane::Diff => {
                self.scroll_detail_page(12);
                return;
            }
        };
        self.detail_title = title.to_string();
        self.detail = vec![line];
        self.detail_scroll = 0;
        self.pane = UiPane::Diff;
        self.status = format!("opened {title} detail");
    }

    fn set_preview(&mut self, title: &str, args: &[&str]) {
        self.detail_title = title.to_string();
        self.detail = run_self(args).lines().map(str::to_string).collect();
        if self.detail.is_empty() {
            self.detail.push("(empty preview)".to_string());
        }
        self.detail_scroll = 0;
        self.pane = UiPane::Diff;
        self.status = format!("rendered {title}");
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
                .constraints([Constraint::Min(3), Constraint::Length(3)])
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
            let left = Layout::default()
                .direction(Direction::Vertical)
                .constraints([Constraint::Min(5), Constraint::Length(8)])
                .split(body[0]);
            app.file_visible_rows = middle[1].height.saturating_sub(2).max(1) as usize;
            app.keep_selected_file_visible();

            frame.render_widget(
                selectable_list(
                    &operation_title,
                    &operation_labels,
                    app.selected_operation,
                    0,
                    app.pane == UiPane::Operation || app.pane == UiPane::Repository,
                    Color::Cyan,
                ),
                left[0],
            );
            frame.render_widget(key_pane(), left[1]);
            frame.render_widget(
                selectable_list(
                    &change_title,
                    &change_labels,
                    app.selected_change,
                    0,
                    app.pane == UiPane::Change,
                    Color::Green,
                ),
                middle[0],
            );
            frame.render_widget(
                selectable_list(
                    &files_title,
                    &visible_from(&file_labels, app.file_scroll),
                    app.selected_file,
                    app.file_scroll,
                    app.pane == UiPane::Files,
                    Color::Magenta,
                ),
                middle[1],
            );
            frame.render_widget(
                Paragraph::new(styled_lines(&app.detail))
                    .block(focused_block(
                        app.detail_title.as_str(),
                        app.pane == UiPane::Diff,
                        Color::Yellow,
                    ))
                    .scroll((app.detail_scroll, 0))
                    .wrap(Wrap { trim: false }),
                body[2],
            );
            frame.render_widget(
                Paragraph::new(Line::from(vec![
                    Span::styled(app.context(), Style::default().fg(Color::Cyan)),
                    Span::raw(" | "),
                    Span::styled(app.status.as_str(), Style::default().fg(Color::Green)),
                ]))
                .block(Block::default().borders(Borders::ALL)),
                areas[1],
            );
        })?;

        let event = event::read()?;
        let Event::Key(key) = event else {
            if let Event::Mouse(mouse) = event {
                if matches!(mouse.kind, MouseEventKind::Down(_)) {
                    let size = terminal.size()?;
                    app.focus_at(mouse.column, mouse.row, size.width, size.height);
                }
            }
            continue;
        };
        if key.kind != KeyEventKind::Press {
            continue;
        }
        let action = match key.code {
            KeyCode::Char('1') => {
                app.pane = UiPane::Operation;
                app.status = "pane: Operation".to_string();
                continue;
            }
            KeyCode::Char('2') => {
                app.pane = UiPane::Change;
                app.status = "pane: Change".to_string();
                continue;
            }
            KeyCode::Char('3') => {
                app.pane = UiPane::Files;
                app.status = "pane: Files".to_string();
                continue;
            }
            KeyCode::Char('4') => {
                app.pane = UiPane::Diff;
                app.status = "pane: Diff".to_string();
                continue;
            }
            KeyCode::Char('o') => {
                app.status = "current-terminal editor disabled; use O".to_string();
                continue;
            }
            KeyCode::Char('O') => {
                app.status = match app.selected_editor_file() {
                    Ok(file) => launcher
                        .open_editor(OpenRequest {
                            file,
                            placement: launcher.settings.placement,
                        })
                        .map(|_| "editor launched".to_string())
                        .unwrap_or_else(|error| error),
                    Err(error) => error,
                };
                continue;
            }
            KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.scroll_detail_page(-12);
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
                app.status = format!("pane: {to:?}");
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

fn selectable_list<'a>(
    title: &'a str,
    items: &'a [String],
    selected: usize,
    offset: usize,
    focused: bool,
    color: Color,
) -> List<'a> {
    let rows = items
        .iter()
        .enumerate()
        .map(|(row, item)| {
            let index = offset + row;
            let style = if index == selected {
                Style::default()
                    .fg(Color::Black)
                    .bg(Color::Yellow)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(Span::styled(item.as_str(), style)))
        })
        .collect::<Vec<_>>();

    List::new(rows).block(focused_block(title, focused, color))
}

fn key_pane<'a>() -> Paragraph<'a> {
    let lines = vec![
        Line::from(Span::styled(
            "keys",
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::raw("Tab/S-Tab focus"),
        Line::raw("1/2/3/4 panes"),
        Line::raw("j/k move/scroll"),
        Line::raw("Enter detail"),
        Line::raw("S-Enter back page"),
        Line::raw("O external editor"),
        Line::raw("r refresh u undo"),
        Line::raw("Ctrl-r redo R snapshot q quit"),
    ];

    Paragraph::new(lines)
        .block(Block::default().title("Keys").borders(Borders::ALL))
        .wrap(Wrap { trim: false })
}

fn visible_from(items: &[String], offset: usize) -> Vec<String> {
    items.iter().skip(offset).cloned().collect()
}

impl TerminalLauncher for ConfiguredLauncher {
    fn open_editor(&self, request: OpenRequest) -> Result<(), String> {
        match request.placement {
            OpenPlacement::CurrentTerminal => Err(
                "current-terminal editor disabled in TUI; use new-window/new-pane/new-tab"
                    .to_string(),
            ),
            OpenPlacement::NewWindow => spawn_new_window(&self.settings, &request.file),
            OpenPlacement::NewPane | OpenPlacement::NewTab => {
                if std::env::var_os("TMUX").is_some() {
                    spawn_tmux(&self.settings.command, &request.file, request.placement)
                } else {
                    spawn_new_window(&self.settings, &request.file)
                }
            }
        }
    }
}

fn load_editor_settings() -> EditorSettings {
    let mut settings = EditorSettings {
        command: vec!["fresh".to_string(), "{file}".to_string()],
        placement: OpenPlacement::NewWindow,
        launcher: "ghostty".to_string(),
    };
    for path in config_paths() {
        if let Ok(text) = fs::read_to_string(path) {
            parse_editor_settings(&text, &mut settings);
        }
    }
    settings
}

fn config_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if let Ok(home) = std::env::var("HOME") {
        paths.push(PathBuf::from(home).join(".config/vijutsu/config.toml"));
    }
    paths.push(PathBuf::from("vijutsu.toml"));
    paths
}

fn parse_editor_settings(text: &str, settings: &mut EditorSettings) {
    let mut section = "";
    for raw in text.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.is_empty() {
            continue;
        }
        if line == "[editor]" || line == "[terminal]" {
            section = line;
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        match (section, key.trim()) {
            ("[editor]", "command") => {
                let command = parse_string_array(value);
                if !command.is_empty() {
                    settings.command = command;
                }
            }
            ("[terminal]", "placement") => {
                settings.placement = parse_placement(value).unwrap_or(settings.placement);
            }
            ("[terminal]", "launcher") => {
                if let Some(value) = parse_string(value) {
                    settings.launcher = value;
                }
            }
            _ => {}
        }
    }
}

fn parse_string_array(value: &str) -> Vec<String> {
    value
        .trim()
        .trim_start_matches('[')
        .trim_end_matches(']')
        .split(',')
        .filter_map(parse_string)
        .collect()
}

fn parse_string(value: &str) -> Option<String> {
    let value = value.trim();
    let quoted = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(quoted.to_string())
}

fn parse_placement(value: &str) -> Option<OpenPlacement> {
    match parse_string(value)?.as_str() {
        "current-terminal" => Some(OpenPlacement::CurrentTerminal),
        "new-pane" => Some(OpenPlacement::NewPane),
        "new-tab" => Some(OpenPlacement::NewTab),
        "new-window" => Some(OpenPlacement::NewWindow),
        _ => None,
    }
}

fn editor_argv(command: &[String], file: &str) -> Result<Vec<String>, String> {
    if command.is_empty() {
        return Err("editor command is empty".to_string());
    }
    Ok(command
        .iter()
        .map(|part| part.replace("{file}", file))
        .collect())
}

fn run_editor_command(command: &[String], file: &str) -> Result<(), String> {
    let argv = editor_argv(command, file)?;
    let status = Command::new(&argv[0])
        .args(&argv[1..])
        .status()
        .map_err(|error| format!("failed to run editor {}: {error}", argv[0]))?;
    if status.success() {
        Ok(())
    } else {
        Err(format!("editor exited with {status}"))
    }
}

fn spawn_new_window(settings: &EditorSettings, file: &str) -> Result<(), String> {
    let argv = editor_argv(&settings.command, file)?;
    let mut command = if settings.launcher == "ghostty" && cfg!(target_os = "macos") {
        let mut command = Command::new("open");
        command.args(["-na", "Ghostty.app", "--args", "-e"]);
        command
    } else if settings.launcher == "ghostty" {
        let mut command = Command::new(&settings.launcher);
        command.arg("-e");
        command
    } else {
        let mut command = Command::new(&settings.launcher);
        command.arg("-e");
        command
    };
    command.args(&argv);
    command
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to launch {}: {error}", settings.launcher))
}

fn spawn_tmux(command: &[String], file: &str, placement: OpenPlacement) -> Result<(), String> {
    let argv = editor_argv(command, file)?;
    let joined = argv.join(" ");
    let mut tmux = Command::new("tmux");
    match placement {
        OpenPlacement::NewPane => {
            tmux.args(["split-window", &joined]);
        }
        OpenPlacement::NewTab => {
            tmux.args(["new-window", &joined]);
        }
        _ => return run_editor_command(command, file),
    }
    tmux.spawn()
        .map(|_| ())
        .map_err(|error| format!("failed to launch tmux: {error}"))
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
        let mut markers = String::new();
        if record.is_current {
            markers.push('@');
        }
        if is_head {
            markers.push('◆');
        }
        if markers.is_empty() {
            markers.push(' ');
        }
        let description = if is_snapshot {
            "snapshot working copy".to_string()
        } else if record.description.is_empty() {
            "(no description)".to_string()
        } else {
            record.description
        };
        operations.push(TuiOperation {
            id: record.operation_id.clone(),
            label: format!("{markers} {} {description}", short(&record.operation_id)),
            is_current: record.is_current,
        });
    }
    if skipped_snapshots > 0 {
        if let Some(previous) = operations.last_mut() {
            previous
                .label
                .push_str(&format!(" (+{skipped_snapshots} snapshots)"));
        }
    }

    Ok((version, current_head, operations))
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
            let label = if root {
                "ROOT [immutable]".to_string()
            } else {
                let mut flags = Vec::new();
                if !record.working_copies.is_empty() {
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
                let suffix = if flags.is_empty() {
                    String::new()
                } else {
                    format!(" [{}]", flags.join(" "))
                };
                format!(
                    "○ {} {} {}{}",
                    short(&record.change_id),
                    short(&record.commit_id),
                    record.description,
                    suffix
                )
            };
            TuiChange {
                change_id: record.change_id,
                commit_id: record.commit_id,
                label,
            }
        })
        .collect())
}

fn fail_tui_parse<T>(context: &str, result: &ParseResult<T>) -> Result<(), String> {
    parse_diagnostic(context, &result.errors, &result.raw_stdout)
        .map(Err)
        .unwrap_or(Ok(()))
}

fn focused_block<'a>(title: &'a str, focused: bool, color: Color) -> Block<'a> {
    let style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(color)
    };
    Block::default()
        .title(Span::styled(title, style.add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_style(style)
}

fn styled_lines(lines: &[String]) -> Vec<Line<'_>> {
    lines
        .iter()
        .map(|line| {
            let color = if line.starts_with('+') {
                Color::Green
            } else if line.starts_with('-') {
                Color::Red
            } else if line.starts_with("@@") {
                Color::Cyan
            } else if line.starts_with("warning:") {
                Color::Yellow
            } else {
                Color::White
            };
            Line::from(Span::styled(line.as_str(), Style::default().fg(color)))
        })
        .collect()
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
    fn editor_settings_parse_command_and_terminal() {
        let mut settings = EditorSettings {
            command: vec!["fresh".to_string(), "{file}".to_string()],
            placement: OpenPlacement::NewWindow,
            launcher: "ghostty".to_string(),
        };

        parse_editor_settings(
            r#"
[editor]
command = ["nvim", "{file}"]

[terminal]
placement = "new-pane"
launcher = "tmux"
"#,
            &mut settings,
        );

        assert_eq!(settings.command, vec!["nvim", "{file}"]);
        assert_eq!(settings.placement, OpenPlacement::NewPane);
        assert_eq!(settings.launcher, "tmux");
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
