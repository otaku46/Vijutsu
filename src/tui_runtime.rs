use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Borders, List, ListItem};

pub fn selectable_list<'a>(
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
                selected_row_style(focused)
            } else {
                Style::default().fg(color)
            };
            ListItem::new(Line::from(Span::styled(item.as_str(), style)))
        })
        .collect::<Vec<_>>();

    List::new(rows).block(focused_block(title, focused, color))
}

pub fn focused_block<'a>(title: &'a str, focused: bool, color: Color) -> Block<'a> {
    let style = if focused {
        Style::default().fg(Color::Yellow)
    } else {
        Style::default().fg(color)
    };
    Block::default()
        .title(Span::styled(title, style.add_modifier(Modifier::BOLD)))
        .borders(Borders::ALL)
        .border_type(if focused {
            BorderType::Thick
        } else {
            BorderType::Plain
        })
        .border_style(style)
}

pub fn styled_lines(lines: &[String]) -> Vec<Line<'_>> {
    lines
        .iter()
        .map(|line| {
            Line::from(Span::styled(
                line.as_str(),
                Style::default().fg(line_color(line)),
            ))
        })
        .collect()
}

fn line_color(line: &str) -> Color {
    if line.starts_with('+') || line.starts_with('✓') {
        Color::Green
    } else if line.starts_with('-') || line.starts_with('✗') {
        Color::Red
    } else if line.starts_with("@@") {
        Color::Cyan
    } else if line.starts_with("warning:") {
        Color::Yellow
    } else {
        Color::White
    }
}

const CONFIRM_PROMPT: &str = "Press y to execute, Esc or N to cancel.";
const OP_ID_LABELS: [&str; 4] = ["current op:", "viewing op:", "source op:", "target op:"];
const CHANGE_ID_LABELS: [&str; 8] = [
    "source commit:",
    "source:",
    "destination:",
    "selected:",
    "parent:",
    "abandoned:",
    "changes-in:",
    "target:",
];
const SECTION_HEADERS: [&str; 8] = [
    "Target:",
    "Change graph:",
    "Operation graph:",
    "Working copy:",
    "Bookmarks:",
    "Remote tracking:",
    "Recoverability:",
    "Command:",
];

pub fn overlay_lines(lines: &[String]) -> Vec<Line<'_>> {
    lines.iter().map(|line| overlay_line(line)).collect()
}

fn overlay_line(line: &str) -> Line<'_> {
    let trimmed = line.trim_start();
    let indent = line.len() - trimmed.len();
    if trimmed == CONFIRM_PROMPT {
        let text = Style::default().fg(Color::Yellow);
        let execute = Style::default()
            .fg(Color::Blue)
            .add_modifier(Modifier::BOLD);
        let cancel = Style::default().fg(Color::Red).add_modifier(Modifier::BOLD);
        return Line::from(vec![
            Span::raw(&line[..indent]),
            Span::styled("Press ", text),
            Span::styled("y", execute),
            Span::styled(" to execute, ", text),
            Span::styled("Esc", cancel),
            Span::styled(" or ", text),
            Span::styled("N", cancel),
            Span::styled(" to cancel.", text),
        ]);
    }
    for label in OP_ID_LABELS {
        if trimmed.starts_with(label) {
            return labeled_line(line, indent + label.len(), Color::Cyan, false);
        }
    }
    if indent == 0 {
        for header in SECTION_HEADERS {
            if trimmed.starts_with(header) {
                return labeled_line(line, header.len(), Color::White, true);
            }
        }
    }
    for label in CHANGE_ID_LABELS {
        if trimmed.starts_with(label) {
            return labeled_line(line, indent + label.len(), Color::Green, false);
        }
    }
    Line::from(Span::styled(line, Style::default().fg(line_color(line))))
}

fn labeled_line(line: &str, split: usize, value_color: Color, bold_label: bool) -> Line<'_> {
    let label_style = if bold_label {
        Style::default()
            .fg(Color::White)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::White)
    };
    Line::from(vec![
        Span::styled(&line[..split], label_style),
        Span::styled(&line[split..], Style::default().fg(value_color)),
    ])
}

pub fn styled_diff_lines(lines: &[String]) -> Vec<Line<'_>> {
    styled_lines(lines)
}

pub fn highlighted_diff_lines<'a>(lines: &'a [String], _path: &str) -> Vec<Line<'a>> {
    styled_diff_lines(lines)
}

pub fn diff_side_lines(lines: &[String], additions: bool) -> Vec<String> {
    let own_prefix = if additions { '+' } else { '-' };
    let other_prefix = if additions { '-' } else { '+' };
    let own_header = if additions { "+++" } else { "---" };
    let other_header = if additions { "---" } else { "+++" };
    let side = lines
        .iter()
        .filter(|line| {
            line.starts_with(own_prefix) && !line.starts_with(own_header)
                || !line.starts_with(other_prefix)
                || line.starts_with(other_header)
        })
        .cloned()
        .collect::<Vec<_>>();
    if side.is_empty() {
        vec!["(none)".to_string()]
    } else {
        side
    }
}

pub fn visible_from(items: &[String], offset: usize) -> Vec<String> {
    items.iter().skip(offset).cloned().collect()
}

fn selected_row_style(focused: bool) -> Style {
    if focused {
        Style::default()
            .fg(Color::Black)
            .bg(Color::Yellow)
            .add_modifier(Modifier::UNDERLINED)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::default().fg(Color::Black).bg(Color::Gray)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlay_line_colors_prompt_ids_and_headers() {
        let prompt = overlay_line("Press y to execute, Esc or N to cancel.");
        assert_eq!(prompt.spans.len(), 8);
        assert_eq!(prompt.spans[2].content, "y");
        assert_eq!(prompt.spans[2].style.fg, Some(Color::Blue));
        assert_eq!(prompt.spans[4].content, "Esc");
        assert_eq!(prompt.spans[4].style.fg, Some(Color::Red));

        let op = overlay_line("current op: abc12345");
        assert_eq!(op.spans[1].style.fg, Some(Color::Cyan));

        let change = overlay_line("  source: abc12345 def67890");
        assert_eq!(change.spans[1].style.fg, Some(Color::Green));

        let header = overlay_line("Operation graph: +1 squash operation");
        assert_eq!(header.spans[0].content, "Operation graph:");
        assert!(header.spans[0].style.add_modifier.contains(Modifier::BOLD));
    }

    #[test]
    fn diff_side_lines_split_added_and_removed_without_headers() {
        let lines = vec![
            "diff --git a/a.txt b/a.txt".to_string(),
            "--- a/a.txt".to_string(),
            "+++ b/a.txt".to_string(),
            "@@ -1 +1 @@".to_string(),
            " context".to_string(),
            "-old".to_string(),
            "+new".to_string(),
        ];

        assert_eq!(
            diff_side_lines(&lines, false),
            vec![
                "diff --git a/a.txt b/a.txt",
                "--- a/a.txt",
                "+++ b/a.txt",
                "@@ -1 +1 @@",
                " context",
                "-old"
            ]
        );
        assert_eq!(
            diff_side_lines(&lines, true),
            vec![
                "diff --git a/a.txt b/a.txt",
                "--- a/a.txt",
                "+++ b/a.txt",
                "@@ -1 +1 @@",
                " context",
                "+new"
            ]
        );
    }
}
