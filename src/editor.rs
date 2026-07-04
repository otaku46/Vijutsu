use std::fs;
use std::path::PathBuf;
use std::process::Command;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OpenPlacement {
    CurrentTerminal,
    NewPane,
    NewTab,
    NewWindow,
}

pub struct OpenRequest {
    pub file: String,
    pub placement: OpenPlacement,
}

pub trait TerminalLauncher {
    fn open_editor(&self, request: OpenRequest) -> Result<(), String>;
}

pub struct EditorSettings {
    pub command: Vec<String>,
    pub placement: OpenPlacement,
    pub launcher: String,
}

pub struct ConfiguredLauncher {
    pub settings: EditorSettings,
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

pub fn load_editor_settings() -> EditorSettings {
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
    let mut section = String::new();
    let mut pending = String::new();
    for raw in text.lines() {
        let stripped = strip_comment(raw).trim().to_string();
        if stripped.is_empty() {
            continue;
        }
        if !pending.is_empty() {
            pending.push(' ');
            pending.push_str(&stripped);
        } else {
            pending = stripped;
        }
        if bracket_delta(&pending) > 0 {
            continue;
        }
        let line = pending.trim();
        if line.is_empty() {
            pending.clear();
            continue;
        }
        if line == "[editor]" || line == "[terminal]" {
            section = line.to_string();
            pending.clear();
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            pending.clear();
            continue;
        };
        match (section.as_str(), key.trim()) {
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
        pending.clear();
    }
}

fn parse_string_array(value: &str) -> Vec<String> {
    split_array_items(value)
        .into_iter()
        .filter_map(|item| parse_string(&item))
        .collect()
}

fn parse_string(value: &str) -> Option<String> {
    let value = value.trim();
    let quoted = value.strip_prefix('"')?.strip_suffix('"')?;
    Some(quoted.to_string())
}

fn strip_comment(raw: &str) -> &str {
    let mut in_quote = false;
    let mut escaped = false;
    for (index, ch) in raw.char_indices() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            '#' if !in_quote => return &raw[..index],
            _ => {}
        }
    }
    raw
}

fn bracket_delta(value: &str) -> isize {
    let mut in_quote = false;
    let mut escaped = false;
    let mut delta = 0;
    for ch in value.chars() {
        if escaped {
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => escaped = true,
            '"' => in_quote = !in_quote,
            '[' if !in_quote => delta += 1,
            ']' if !in_quote => delta -= 1,
            _ => {}
        }
    }
    delta
}

fn split_array_items(value: &str) -> Vec<String> {
    let value = value.trim().trim_start_matches('[').trim_end_matches(']');
    let mut items = Vec::new();
    let mut current = String::new();
    let mut in_quote = false;
    let mut escaped = false;
    for ch in value.chars() {
        if escaped {
            current.push(ch);
            escaped = false;
            continue;
        }
        match ch {
            '\\' if in_quote => {
                current.push(ch);
                escaped = true;
            }
            '"' => {
                current.push(ch);
                in_quote = !in_quote;
            }
            ',' if !in_quote => {
                items.push(current.trim().to_string());
                current.clear();
            }
            _ => current.push(ch),
        }
    }
    if !current.trim().is_empty() {
        items.push(current.trim().to_string());
    }
    items
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn editor_settings_parse_multiline_and_quoted_punctuation() {
        let mut settings = EditorSettings {
            command: vec!["fresh".to_string(), "{file}".to_string()],
            placement: OpenPlacement::NewWindow,
            launcher: "ghostty".to_string(),
        };

        parse_editor_settings(
            r#"
[editor]
command = [
  "fresh",
  "--title=a,b#c",
  "{file}",
]

[terminal]
launcher = "ghostty#nightly"
"#,
            &mut settings,
        );

        assert_eq!(settings.command, vec!["fresh", "--title=a,b#c", "{file}"]);
        assert_eq!(settings.launcher, "ghostty#nightly");
    }
}
