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
