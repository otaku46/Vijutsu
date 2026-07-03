use std::collections::HashSet;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphCharset {
    Unicode,
    Ascii,
}

impl GraphCharset {
    pub fn from_env() -> Self {
        match std::env::var("VIJUTSU_ASCII_GRAPH") {
            Ok(value) if value == "1" || value.eq_ignore_ascii_case("true") => Self::Ascii,
            _ => Self::Unicode,
        }
    }

    fn vertical(self) -> char {
        match self {
            GraphCharset::Unicode => '│',
            GraphCharset::Ascii => '|',
        }
    }

    fn horizontal(self) -> char {
        match self {
            GraphCharset::Unicode => '─',
            GraphCharset::Ascii => '-',
        }
    }

    fn merge(self) -> char {
        match self {
            GraphCharset::Unicode => '┬',
            GraphCharset::Ascii => '+',
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphNode {
    pub id: String,
    pub parent_ids: Vec<String>,
    pub marker: char,
    pub text: String,
}

pub fn render_graph_rows(nodes: &[GraphNode], charset: GraphCharset) -> Vec<String> {
    let visible = nodes
        .iter()
        .map(|node| node.id.as_str())
        .collect::<HashSet<_>>();
    let mut lanes: Vec<Option<String>> = Vec::new();
    let mut rows = Vec::with_capacity(nodes.len());

    for node in nodes {
        let lane = match lanes
            .iter()
            .position(|id| id.as_deref() == Some(node.id.as_str()))
        {
            Some(index) => index,
            None => {
                let index = lanes
                    .iter()
                    .position(Option::is_none)
                    .unwrap_or(lanes.len());
                if index == lanes.len() {
                    lanes.push(Some(node.id.clone()));
                } else {
                    lanes[index] = Some(node.id.clone());
                }
                index
            }
        };

        let mut prefix = String::new();
        for (index, active) in lanes.iter().enumerate() {
            if index == lane {
                prefix.push(node.marker);
            } else if active.is_some() {
                prefix.push(charset.vertical());
            } else {
                prefix.push(' ');
            }
            prefix.push(' ');
        }
        if node.parent_ids.len() > 1 {
            prefix.push(charset.horizontal());
            prefix.push(charset.merge());
            prefix.push(' ');
        }

        rows.push(format!("{prefix}{}", node.text));

        let parents = node
            .parent_ids
            .iter()
            .filter(|parent| visible.contains(parent.as_str()))
            .cloned()
            .collect::<Vec<_>>();
        if parents.is_empty() {
            lanes[lane] = None;
        } else {
            lanes[lane] = Some(parents[0].clone());
            for parent in parents.iter().skip(1) {
                if lanes
                    .iter()
                    .all(|active| active.as_deref() != Some(parent.as_str()))
                {
                    lanes.push(Some(parent.clone()));
                }
            }
        }
        while lanes.last().is_some_and(Option::is_none) {
            lanes.pop();
        }
    }

    rows
}

#[cfg(test)]
mod tests {
    use super::*;

    fn node(id: &str, parents: &[&str]) -> GraphNode {
        GraphNode {
            id: id.to_string(),
            parent_ids: parents.iter().map(|parent| parent.to_string()).collect(),
            marker: '○',
            text: id.to_string(),
        }
    }

    #[test]
    fn renders_linear_chain() {
        let rows = render_graph_rows(
            &[node("c", &["b"]), node("b", &["a"]), node("a", &[])],
            GraphCharset::Unicode,
        );

        assert_eq!(rows, vec!["○ c", "○ b", "○ a"]);
    }

    #[test]
    fn renders_branch_and_merge_deterministically() {
        let nodes = [
            node("m", &["b", "c"]),
            node("b", &["a"]),
            node("c", &["a"]),
            node("a", &[]),
        ];

        let first = render_graph_rows(&nodes, GraphCharset::Unicode);
        let second = render_graph_rows(&nodes, GraphCharset::Unicode);

        assert_eq!(first, second);
        assert!(first[0].contains("─┬"));
        assert!(first.iter().any(|row| row.contains('│')));
    }

    #[test]
    fn supports_ascii_charset() {
        let rows = render_graph_rows(&[node("m", &["a", "b"])], GraphCharset::Ascii);

        assert!(rows[0].contains("-+"));
    }

    #[test]
    fn missing_parent_outside_visible_range_does_not_panic() {
        let rows = render_graph_rows(&[node("b", &["missing"])], GraphCharset::Unicode);

        assert_eq!(rows, vec!["○ b"]);
    }

    #[test]
    fn selected_marker_is_preserved_by_renderer() {
        let rows = render_graph_rows(
            &[GraphNode {
                id: "head".to_string(),
                parent_ids: Vec::new(),
                marker: '@',
                text: "selected".to_string(),
            }],
            GraphCharset::Unicode,
        );

        assert!(rows[0].starts_with("@ "));
    }
}
