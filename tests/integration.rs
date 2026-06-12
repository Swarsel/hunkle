use std::fs;
use std::path::Path;
use std::process::Command;

use hunkle::backend;
use hunkle::model::{build_plan, empty_assignments};

fn git(dir: &Path, args: &[&str]) -> String {
    let out = Command::new("git")
        .arg("-C")
        .arg(dir)
        .args(args)
        .output()
        .expect("failed to run git");
    assert!(
        out.status.success(),
        "git {:?} failed: {}",
        args,
        String::from_utf8_lossy(&out.stderr)
    );
    String::from_utf8_lossy(&out.stdout).into_owned()
}

fn setup_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.name", "test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

fn changed_lines(file: &hunkle::diff::FileDiff, hunk: usize) -> Vec<usize> {
    file.hunks[hunk]
        .lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change())
        .map(|(i, _)| i)
        .collect()
}

#[test]
fn splits_staged_changes_into_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
    fs::write(dir.join("f.txt"), &base).unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);

    let modified = base
        .replace("line2\n", "LINE2\n")
        .replace("line18\n", "LINE18\n");
    fs::write(dir.join("f.txt"), &modified).unwrap();
    fs::write(dir.join("new.txt"), "alpha\nbeta\n").unwrap();
    git(dir, &["add", "-A"]);

    let backend = backend::create("git", dir).unwrap();
    let state = backend.read_state().unwrap();
    assert_eq!(state.files.len(), 2);
    assert_eq!(state.files[0].path, "f.txt");
    assert_eq!(state.files[0].hunks.len(), 2);
    assert_eq!(state.files[1].path, "new.txt");

    let mut assign = empty_assignments(&state.files);
    for li in changed_lines(&state.files[0], 0) {
        assign[0][0][li] = Some(0);
    }
    for li in changed_lines(&state.files[0], 1) {
        assign[0][1][li] = Some(1);
    }
    let new_lines = changed_lines(&state.files[1], 0);
    assert_eq!(new_lines.len(), 2);
    assign[1][0][new_lines[0]] = Some(0);

    let messages = vec!["first".to_string(), "second".to_string()];
    let plan = build_plan(&state.files, &assign, &state.bases, &messages);
    assert_eq!(plan.len(), 2);

    let ids = backend.create_commits(&plan).unwrap();
    assert_eq!(ids.len(), 2);

    let log = git(dir, &["log", "--format=%s"]);
    assert_eq!(
        log.trim().lines().collect::<Vec<_>>(),
        vec!["second", "first", "base"]
    );

    let first = git(dir, &["show", &format!("{}:f.txt", ids[0])]);
    assert!(first.contains("LINE2"));
    assert!(first.contains("line18"));
    let head_f = git(dir, &["show", "HEAD:f.txt"]);
    assert_eq!(head_f, modified);
    let head_new = git(dir, &["show", "HEAD:new.txt"]);
    assert_eq!(head_new, "alpha\n");

    let staged = git(dir, &["diff", "--cached"]);
    assert!(staged.contains("+beta"));
    assert!(!staged.contains("LINE2"));
    let unstaged = git(dir, &["diff"]);
    assert_eq!(unstaged, "");
}

#[test]
fn deleted_file_split_across_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    fs::write(dir.join("gone.txt"), "keep\ndrop\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
    fs::remove_file(dir.join("gone.txt")).unwrap();
    git(dir, &["add", "-A"]);

    let backend = backend::create("git", dir).unwrap();
    let state = backend.read_state().unwrap();
    let lines = changed_lines(&state.files[0], 0);
    let mut assign = empty_assignments(&state.files);
    assign[0][0][lines[0]] = Some(0);
    assign[0][0][lines[1]] = Some(1);

    let messages = vec!["remove keep".to_string(), "remove rest".to_string()];
    let plan = build_plan(&state.files, &assign, &state.bases, &messages);
    let ids = backend.create_commits(&plan).unwrap();

    let partial = git(dir, &["show", &format!("{}:gone.txt", ids[0])]);
    assert_eq!(partial, "drop\n");
    let tree = git(dir, &["ls-tree", "--name-only", "HEAD"]);
    assert!(!tree.contains("gone.txt"));
    assert_eq!(git(dir, &["diff", "--cached"]), "");
}

#[test]
fn unborn_branch_creates_first_commits() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    git(dir, &["add", "-A"]);

    let backend = backend::create("git", dir).unwrap();
    let state = backend.read_state().unwrap();
    let mut assign = empty_assignments(&state.files);
    for li in changed_lines(&state.files[0], 0) {
        assign[0][0][li] = Some(0);
    }
    let messages = vec!["initial".to_string()];
    let plan = build_plan(&state.files, &assign, &state.bases, &messages);
    backend.create_commits(&plan).unwrap();

    let log = git(dir, &["log", "--format=%s"]);
    assert_eq!(log.trim(), "initial");
    assert_eq!(git(dir, &["diff", "--cached"]), "");
}
