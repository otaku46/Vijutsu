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
            let color = if line.starts_with('+') || line.starts_with('✓') {
                Color::Green
            } else if line.starts_with('-') || line.starts_with('✗') {
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
