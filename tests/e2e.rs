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
        let path = (0..100)
            .map(|index| {
                std::env::temp_dir().join(format!(
                    "vijutsu-e2e-{}-{nonce}-{index}",
                    std::process::id()
                ))
            })
            .find(|path| std::fs::create_dir(path).is_ok())
            .expect("failed to create unique temp repo directory");
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

    fn jj(&self, args: &[&str]) -> Output {
        Command::new("jj")
            .args(args)
            .current_dir(&self.path)
            .output()
            .unwrap()
    }

    fn append(&self, path: &str, text: &str) {
        use std::io::Write;

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path.join(path))
            .unwrap();
        file.write_all(text.as_bytes()).unwrap();
    }

    fn add_rireki_change(&self, message: &str, text: &str) -> String {
        let output = self.jj(&["new", "-m", message]);
        assert_success("jj new rireki change", &output);
        self.append("rireki.txt", text);
        let output = self.jj(&["status"]);
        assert_success("jj status snapshot rireki change", &output);
        current_commit(&self.path)
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

fn current_commit(repo: &Path) -> String {
    let output = Command::new("jj")
        .args([
            "--ignore-working-copy",
            "log",
            "--no-graph",
            "-r",
            "@",
            "-T",
            "commit_id ++ \"\\n\"",
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert_success("jj log current commit", &output);
    stdout(&output).trim().to_string()
}

fn file_show(repo: &Path, revision: &str, path: &str) -> String {
    let output = Command::new("jj")
        .args([
            "--ignore-working-copy",
            "file",
            "show",
            "-r",
            revision,
            path,
        ])
        .current_dir(repo)
        .output()
        .unwrap();
    assert_success("jj file show", &output);
    stdout(&output)
}

fn short(id: &str) -> &str {
    id.get(..8).unwrap_or(id)
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
        "B: set or move local bookmark",
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

#[test]
fn bookmarks_command_lists_local_bookmark() {
    let repo = TempRepo::new();
    let _ = repo.add_rireki_change("rireki:a", "a\n");
    let set = repo.jj(&["bookmark", "set", "feature", "-r", "@"]);
    assert_success("jj bookmark set feature", &set);

    let output = repo.run(&["bookmarks"]);

    assert_success("bookmarks", &output);
    assert!(stdout(&output).contains("feature"));
}

#[test]
fn inspect_shows_bookmark_on_change() {
    let repo = TempRepo::new();
    let commit = repo.add_rireki_change("rireki:a", "a\n");
    let set = repo.jj(&["bookmark", "set", "feature", "-r", &commit]);
    assert_success("jj bookmark set feature", &set);

    let output = repo.run(&["inspect", "--change", &commit]);

    assert_success("inspect bookmark", &output);
    let out = stdout(&output);
    assert!(out.contains("bookmark:feature"), "{out}");
}

#[test]
fn rireki_fixture_selected_change_files_diff_and_materialized_content_match() {
    let repo = TempRepo::new();
    repo.add_rireki_change("rireki:a", "a\n");
    let commit = repo.add_rireki_change("rireki:B", "B\n");

    let output = repo.run(&["inspect", "--change", &commit]);

    assert_success("inspect selected rireki:B", &output);
    let out = stdout(&output);
    assert!(out.contains("Files\n"), "{out}");
    assert!(out.contains("rireki.txt"), "{out}");
    assert!(out.contains("Diff\n"), "{out}");
    assert!(
        out.contains("diff --git a/rireki.txt b/rireki.txt"),
        "{out}"
    );
    assert!(out.contains("+B"), "{out}");
    assert_eq!(file_show(&repo.path, &commit, "rireki.txt"), "a\nB\n");
}

#[test]
fn follow_current_inspect_observes_latest_operation_after_external_jj_change() {
    let repo = TempRepo::new();
    let before = current_operation(&repo.path);
    let _ = repo.add_rireki_change("rireki:a", "a\n");
    let after = current_operation(&repo.path);
    assert_ne!(before, after);

    let output = repo.run(&["inspect", "--limit", "5"]);

    assert_success("inspect follows current operation", &output);
    let out = stdout(&output);
    assert!(out.contains(short(&after)), "{out}");
}
