use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::time::{SystemTime, UNIX_EPOCH};

fn bin() -> &'static str {
    env!("CARGO_BIN_EXE_vijutsu")
}

struct TempRepo {
    path: PathBuf,
}

impl TempRepo {
    fn new() -> Self {
        let nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        let path = std::env::temp_dir().join(format!("vijutsu-e2e-{}-{nonce}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        let init = Command::new("jj")
            .args(["git", "init", "."])
            .current_dir(&path)
            .output()
            .unwrap();
        assert_success("jj git init", &init);
        Self { path }
    }

    fn run(&self, args: &[&str]) -> Output {
        Command::new(bin())
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }
}

impl Drop for TempRepo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.path);
    }
}

fn assert_success(context: &str, output: &Output) {
    assert!(
        output.status.success(),
        "{context} failed\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn assert_failure(context: &str, output: &Output) {
    assert!(
        !output.status.success(),
        "{context} unexpectedly succeeded\nstdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

fn stdout(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).into_owned()
}

fn stderr(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

fn current_operation(repo: &Path) -> String {
    let output = Command::new("jj")
        .args([
            "--ignore-working-copy",
            "op",
            "log",
            "--no-graph",
            "--limit",
            "1",
            "-T",
            "id ++ \"\\n\"",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert_success("jj op log current id", &output);
    stdout(&output).trim().to_string()
}

#[test]
fn inspect_limit_one_reports_limit_warning() {
    let repo = TempRepo::new();

    let output = repo.run(&["inspect", "--limit", "1"]);

    assert_success("inspect --limit 1", &output);
    assert!(stdout(&output).contains("Operation limit reached: showing 1 operations"));
}

#[test]
fn previews_render_expected_operation_targets() {
    let repo = TempRepo::new();
    let op = current_operation(&repo.path);

    let undo = repo.run(&["preview", "undo"]);
    assert_success("preview undo", &undo);
    let undo_out = stdout(&undo);
    assert!(undo_out.contains("Preview: Undo"));
    assert!(undo_out.contains("Change graph:"));
    assert!(undo_out.contains("Operation graph:"));
    assert!(undo_out.contains("Working copy:"));
    assert!(undo_out.contains("Bookmarks:"));
    assert!(undo_out.contains("Remote tracking:"));
    assert!(undo_out.contains("Recoverability:"));

    let restore = repo.run(&["preview", "restore", &op]);
    assert_success("preview restore", &restore);
    assert!(stdout(&restore).contains("Preview: Operation restore"));

    let revert = repo.run(&["preview", "revert", &op]);
    assert_success("preview revert", &revert);
    assert!(stdout(&revert).contains("Preview: Operation revert"));
}

#[test]
fn mutate_undo_requires_execute_and_reports_heads_after_execution() {
    let repo = TempRepo::new();

    let rejected = repo.run(&["mutate", "undo"]);
    assert_failure("mutate undo without execute", &rejected);
    assert!(stderr(&rejected).contains("mutation requires --execute"));

    let executed = repo.run(&["mutate", "undo", "--execute"]);
    assert_success("mutate undo --execute", &executed);
    let out = stdout(&executed);
    assert!(out.contains("command: jj"));
    assert!(out.contains("exit: 0"));
    assert!(out.contains("operation heads after mutation:"));
    assert!(out.contains("Operation heads:"));

    let heads = repo.run(&["heads"]);
    assert_success("heads after mutate", &heads);
    assert!(stdout(&heads).contains("Operation heads:"));
}

#[test]
fn mutate_abandon_execute_is_rejected() {
    let repo = TempRepo::new();

    let output = repo.run(&["mutate", "abandon", "@", "--execute"]);

    assert_failure("mutate abandon --execute", &output);
    assert!(stderr(&output).contains("operation abandon is preview-only"));
}

#[test]
fn keys_lists_supported_bindings() {
    let repo = TempRepo::new();

    let output = repo.run(&["keys"]);

    assert_success("keys", &output);
    let out = stdout(&output);
    for expected in [
        "j: next",
        "k: previous",
        "Tab: switch pane",
        "Ctrl-r: redo preview",
        "q: quit",
    ] {
        assert!(out.contains(expected), "missing {expected} in\n{out}");
    }
}

#[test]
fn help_lists_tui_command() {
    let repo = TempRepo::new();

    let output = repo.run(&["help"]);

    assert_success("help", &output);
    assert!(stdout(&output).contains("  tui"));
}
