use std::io::{self, Write};

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
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Wrap};

use crate::editor::{ConfiguredLauncher, OpenRequest, TerminalLauncher, load_editor_settings};
use crate::tui::{KeyAction, UiCommand, UiPane, handle_key_input};
use crate::tui_app::{ActiveModal, TuiApp, UiMode};
use crate::tui_runtime::{
    diff_side_lines, focused_block, highlighted_diff_lines, overlay_lines, selectable_list,
    visible_from,
};
use crate::tui_support::{centered_content_rect, is_command_modifier};

pub fn run_tui_or_exit() {
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
    execute!(
        terminal.backend_mut(),
        DisableMouseCapture,
        LeaveAlternateScreen
    )?;
    terminal.backend_mut().flush()?;
    disable_raw_mode()?;
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
                .constraints(if app.operation_pane_collapsed {
                    [
                        Constraint::Length(0),
                        Constraint::Percentage(36),
                        Constraint::Percentage(64),
                    ]
                } else {
                    [
                        Constraint::Percentage(28),
                        Constraint::Percentage(28),
                        Constraint::Percentage(44),
                    ]
                })
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
                let diff_path = app.selected_file_path();
                if app.maximized_pane == Some(UiPane::Diff) {
                    let diff_name = if diff_path.is_empty() {
                        "diff"
                    } else {
                        diff_path
                    };
                    let removed_title = format!("{diff_name} removed");
                    let added_title = format!("{diff_name} added");
                    let panes = Layout::default()
                        .direction(Direction::Horizontal)
                        .constraints([Constraint::Percentage(50), Constraint::Percentage(50)])
                        .split(area);
                    let deleted = diff_side_lines(&app.detail, false);
                    let added = diff_side_lines(&app.detail, true);
                    frame.render_widget(
                        Paragraph::new(highlighted_diff_lines(&deleted, diff_path))
                            .block(focused_block(
                                &removed_title,
                                app.pane == UiPane::Diff,
                                Color::Red,
                            ))
                            .scroll((app.detail_scroll, 0))
                            .wrap(Wrap { trim: false }),
                        panes[0],
                    );
                    frame.render_widget(
                        Paragraph::new(highlighted_diff_lines(&added, diff_path))
                            .block(focused_block(
                                &added_title,
                                app.pane == UiPane::Diff,
                                Color::Green,
                            ))
                            .scroll((app.detail_scroll, 0))
                            .wrap(Wrap { trim: false }),
                        panes[1],
                    );
                } else {
                    frame.render_widget(
                        Paragraph::new(highlighted_diff_lines(&app.detail, diff_path))
                            .block(focused_block(
                                app.detail_title.as_str(),
                                app.pane == UiPane::Diff,
                                Color::Yellow,
                            ))
                            .scroll((app.detail_scroll, 0))
                            .wrap(Wrap { trim: false }),
                        area,
                    );
                }
            };

            match app.maximized_pane {
                Some(UiPane::Operation | UiPane::Repository) => render_operation(frame, areas[0]),
                Some(UiPane::Change) => render_change(frame, areas[0]),
                Some(UiPane::Files) => render_files(frame, areas[0]),
                Some(UiPane::Diff) => render_diff(frame, areas[0]),
                None => {
                    if !app.operation_pane_collapsed {
                        render_operation(frame, body[0]);
                    }
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
            if let Some((title, lines, scroll)) = app.modal_overlay() {
                let area = centered_content_rect(&title, &lines, frame.size());
                frame.render_widget(Clear, area);
                frame.render_widget(
                    Paragraph::new(overlay_lines(&lines))
                        .block(focused_block(title.as_str(), true, Color::Yellow))
                        .scroll((scroll, 0))
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
        match &app.modal {
            ActiveModal::Describe { .. } => {
                match key.code {
                    KeyCode::Enter => app.execute_describe_input(),
                    KeyCode::Esc | KeyCode::Char('N') => app.cancel_describe_input(),
                    KeyCode::Backspace => app.pop_describe_char(),
                    KeyCode::Char(ch) => app.push_describe_char(ch),
                    _ => app.status = "describe input: type, Enter apply, Esc/N cancel".to_string(),
                }
                continue;
            }
            ActiveModal::Commit { .. } => {
                match key.code {
                    KeyCode::Enter => app.confirm_commit_input(),
                    KeyCode::Esc | KeyCode::Char('N') => app.cancel_commit_input(),
                    KeyCode::Backspace => app.pop_commit_char(),
                    KeyCode::Char(ch) => app.push_commit_char(ch),
                    _ => app.status = "commit input: type, Enter confirm, Esc/N cancel".to_string(),
                }
                continue;
            }
            ActiveModal::Bookmark { .. } => {
                match key.code {
                    KeyCode::Enter => app.confirm_bookmark_input(),
                    KeyCode::Esc | KeyCode::Char('N') => app.cancel_bookmark_input(),
                    KeyCode::Backspace => app.pop_bookmark_char(),
                    KeyCode::Char(ch) => app.push_bookmark_char(ch),
                    _ => {
                        app.status = "bookmark input: type, Enter confirm, Esc/N cancel".to_string()
                    }
                }
                continue;
            }
            ActiveModal::Workspace { .. } => {
                match key.code {
                    KeyCode::Enter => app.confirm_workspace_input(),
                    KeyCode::Esc => app.cancel_workspace_input(),
                    KeyCode::Backspace => app.pop_workspace_char(),
                    KeyCode::Char(ch) => app.push_workspace_char(ch),
                    _ => {
                        app.status =
                            "workspace input: type path, Enter confirm, Esc cancel".to_string()
                    }
                }
                continue;
            }
            ActiveModal::PendingMutation { .. } => {
                match key.code {
                    KeyCode::Char('y') | KeyCode::Char('Y') => app.execute_pending_mutation(),
                    KeyCode::Esc | KeyCode::Char('N') => app.cancel_pending_mutation(),
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.scroll_overlay_page(3);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.scroll_overlay_page(-3);
                    }
                    _ => {
                        app.status =
                            "pending mutation: press y to execute, Esc/N to cancel".to_string()
                    }
                }
                continue;
            }
            ActiveModal::Rebase { .. } => {
                match key.code {
                    KeyCode::Enter => app.finish_rebase_selection(),
                    KeyCode::Esc | KeyCode::Char('N') => {
                        app.modal = ActiveModal::None;
                        app.refresh_diff();
                        app.status = "rebase selection cancelled".to_string();
                    }
                    KeyCode::Char('j') | KeyCode::Down => app.move_selection(1),
                    KeyCode::Char('k') | KeyCode::Up => app.move_selection(-1),
                    _ => {
                        app.status =
                            "rebase selection: j/k choose destination, Enter confirm, Esc/N cancel"
                                .to_string()
                    }
                }
                continue;
            }
            ActiveModal::Overlay(_) => {
                match key.code {
                    KeyCode::Esc if app.mode == UiMode::Surgery => app.exit_surgery_mode(),
                    KeyCode::Enter if key.modifiers.contains(KeyModifiers::SHIFT) => {
                        app.scroll_overlay_page(-12);
                    }
                    KeyCode::Esc | KeyCode::Enter => {
                        app.clear_overlay();
                        app.set_browse_status("overlay closed");
                    }
                    KeyCode::Char('j') | KeyCode::Down => {
                        app.scroll_overlay_page(3);
                    }
                    KeyCode::Char('k') | KeyCode::Up => {
                        app.scroll_overlay_page(-3);
                    }
                    KeyCode::Char(' ') => {
                        app.scroll_overlay_page(12);
                    }
                    _ => app.status = "overlay: Enter/Esc close, j/k scroll".to_string(),
                }
                continue;
            }
            _ => {}
        }
        let action = match key.code {
            KeyCode::Esc if app.mode == UiMode::Surgery => {
                app.exit_surgery_mode();
                continue;
            }
            _ if app.has_modal() => {
                app.status = "overlay open; Esc closes it".to_string();
                continue;
            }
            KeyCode::Char('1') => {
                app.restore_operation_pane();
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
            KeyCode::Char('W') => {
                app.begin_workspace_input();
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
            KeyCode::Char('l') => {
                app.plan_split_selected();
                continue;
            }
            KeyCode::Char('D') => {
                app.plan_duplicate_selected();
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
            KeyCode::Left if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.collapse_operation_pane();
                continue;
            }
            KeyCode::Right if key.modifiers.contains(KeyModifiers::SHIFT) => {
                app.restore_operation_pane();
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
                app.snapshot_then_refresh();
            }
            UiCommand::Quit => return Ok(()),
            UiCommand::MoveSelection { delta } => app.move_selection(delta),
            UiCommand::Noop => {}
        }
    }
}

fn bottom_key_hint(app: &TuiApp) -> Line<'static> {
    let mut spans = vec![Span::styled("Keys ", Style::default().fg(Color::Green))];
    for (index, (key, label)) in context_key_hint(app).iter().enumerate() {
        if index > 0 {
            spans.push(Span::raw("  "));
        }
        spans.push(Span::styled(
            *key,
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        ));
        spans.push(Span::styled(
            format!(":{label}"),
            Style::default().fg(Color::Gray),
        ));
    }
    Line::from(spans)
}

fn context_key_hint(app: &TuiApp) -> &'static [(&'static str, &'static str)] {
    match &app.modal {
        ActiveModal::Overlay(_) => {
            return &[("Enter/Esc", "close"), ("j/k", "scroll"), ("Space", "page")];
        }
        ActiveModal::Describe { .. }
        | ActiveModal::Commit { .. }
        | ActiveModal::Bookmark { .. } => {
            return &[
                ("Enter", "apply"),
                ("Esc/N", "cancel"),
                ("Backspace", "delete"),
            ];
        }
        ActiveModal::Workspace { .. } => {
            return &[
                ("Enter", "apply"),
                ("Esc", "cancel"),
                ("Backspace", "delete"),
            ];
        }
        ActiveModal::PendingMutation { .. } => {
            return &[("y", "execute"), ("Esc/N", "cancel"), ("j/k", "scroll")];
        }
        ActiveModal::Rebase { .. } => {
            return &[
                ("j/k", "choose destination"),
                ("Enter", "confirm"),
                ("Esc/N", "cancel"),
            ];
        }
        ActiveModal::None => {}
    }
    match (app.mode, app.pane) {
        (UiMode::Surgery, UiPane::Operation | UiPane::Repository) => &[
            ("Tab", "Pane"),
            ("j/k", "Move"),
            ("h", "Graph parent"),
            ("a", "preview abandon"),
            ("Esc", "Browse"),
            ("z", "max"),
            ("S-←/→", "fold op"),
            ("?", "help"),
        ],
        (_, UiPane::Operation | UiPane::Repository) => &[
            ("Tab", "Pane"),
            ("j/k", "Move"),
            ("h", "Graph parent"),
            ("Enter", "detail"),
            ("r", "refresh"),
            ("R", "snapshot"),
            ("S", "Surgery"),
            ("z", "max"),
            ("S-←/→", "fold op"),
            ("?", "help"),
        ],
        (_, UiPane::Change) => &[
            ("Tab", "Pane"),
            ("j/k", "Move"),
            ("h", "Graph parent"),
            ("Space", "Pin"),
            ("e", "edit"),
            ("n", "new"),
            ("l", "split"),
            ("D", "duplicate"),
            ("d", "describe"),
            ("a", "abandon"),
            ("s", "squash"),
            ("b", "rebase"),
            ("Z", "restore"),
            ("z", "max"),
            ("S-←/→", "fold op"),
            ("?", "help"),
        ],
        (_, UiPane::Files) => &[
            ("Tab", "Pane"),
            ("j/k", "Move"),
            ("Shift+↑↓", "range"),
            ("Cmd+click", "toggle"),
            ("Enter", "diff"),
            ("O", "open"),
            ("I", "import"),
            ("r", "refresh"),
            ("z", "max"),
            ("S-←/→", "fold op"),
            ("?", "help"),
        ],
        (_, UiPane::Diff) => &[
            ("Tab", "Pane"),
            ("j/k", "Scroll"),
            ("Enter", "page"),
            ("Shift+Enter", "page up"),
            ("r", "refresh"),
            ("R", "snapshot"),
            ("z", "max"),
            ("S-←/→", "fold op"),
            ("L", "log"),
            ("?", "help"),
        ],
    }
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
