use std::fs;
use std::path::Path;
use std::process::Command;

use hunkle::backend;
use hunkle::model::{CommitSpec, build_plan, empty_assignments};

fn spec(message: &str) -> CommitSpec {
    CommitSpec {
        message: message.to_string(),
        branch: None,
    }
}

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

    let specs = vec![spec("first"), spec("second")];
    let plan = build_plan(&state.files, &assign, &state.bases, &specs);
    assert_eq!(plan.commits.len(), 2);

    let ids = backend.create_commits(&plan).unwrap().ids;
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

    let specs = vec![spec("remove keep"), spec("remove rest")];
    let plan = build_plan(&state.files, &assign, &state.bases, &specs);
    let ids = backend.create_commits(&plan).unwrap().ids;

    let partial = git(dir, &["show", &format!("{}:gone.txt", ids[0])]);
    assert_eq!(partial, "drop\n");
    let tree = git(dir, &["ls-tree", "--name-only", "HEAD"]);
    assert!(!tree.contains("gone.txt"));
    assert_eq!(git(dir, &["diff", "--cached"]), "");
}

#[test]
fn branch_commits_fork_from_head_and_are_unstaged() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    let base: String = (1..=20).map(|i| format!("line{i}\n")).collect();
    fs::write(dir.join("f.txt"), &base).unwrap();
    fs::write(dir.join("g.txt"), "one\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
    let base_sha = git(dir, &["rev-parse", "HEAD"]).trim().to_string();

    let modified = base
        .replace("line2\n", "LINE2\n")
        .replace("line18\n", "LINE18\n");
    fs::write(dir.join("f.txt"), &modified).unwrap();
    fs::write(dir.join("g.txt"), "ONE\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);
    fs::write(dir.join("g.txt"), "ONE\ntwo\nthree\n").unwrap();

    let backend = backend::create("git", dir).unwrap();
    let state = backend.read_state().unwrap();
    assert_eq!(state.files[0].path, "f.txt");
    assert_eq!(state.files[1].path, "g.txt");

    let mut assign = empty_assignments(&state.files);
    for li in changed_lines(&state.files[0], 0) {
        assign[0][0][li] = Some(0);
    }
    for li in changed_lines(&state.files[0], 1) {
        assign[0][1][li] = Some(1);
    }
    for li in changed_lines(&state.files[1], 0) {
        assign[1][0][li] = Some(1);
    }

    let specs = vec![
        spec("local"),
        CommitSpec {
            message: "side fix".to_string(),
            branch: Some("topic".to_string()),
        },
    ];
    let plan = build_plan(&state.files, &assign, &state.bases, &specs);
    let created = backend.create_commits(&plan).unwrap();
    assert_eq!(created.ids.len(), 2);
    assert_eq!(created.worktree_skipped, vec!["g.txt".to_string()]);

    let log = git(dir, &["log", "--format=%s"]);
    assert_eq!(
        log.trim().lines().collect::<Vec<_>>(),
        vec!["local", "base"]
    );
    assert_eq!(
        git(dir, &["rev-parse", "topic^"]).trim(),
        base_sha,
        "topic must fork from the original HEAD"
    );
    assert_eq!(
        git(dir, &["log", "--format=%s", "topic"]).trim(),
        "side fix\nbase"
    );

    let topic_f = git(dir, &["show", "topic:f.txt"]);
    assert!(topic_f.contains("LINE18") && topic_f.contains("line2\n"));
    assert_eq!(git(dir, &["show", "topic:g.txt"]), "ONE\ntwo\n");
    let head_f = git(dir, &["show", "HEAD:f.txt"]);
    assert!(head_f.contains("LINE2") && head_f.contains("line18\n"));

    assert_eq!(git(dir, &["diff", "--cached"]), "");
    let f_worktree = fs::read_to_string(dir.join("f.txt")).unwrap();
    assert!(
        !f_worktree.contains("LINE18"),
        "branch lines must leave the worktree"
    );
    assert_eq!(
        fs::read_to_string(dir.join("g.txt")).unwrap(),
        "ONE\ntwo\nthree\n",
        "files with unstaged edits must be left alone"
    );

    let err = backend.create_commits(&plan).unwrap_err().to_string();
    assert!(err.contains("already exists"), "unexpected error: {err}");
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
    let specs = vec![spec("initial")];
    let plan = build_plan(&state.files, &assign, &state.bases, &specs);
    backend.create_commits(&plan).unwrap();

    let log = git(dir, &["log", "--format=%s"]);
    assert_eq!(log.trim(), "initial");
    assert_eq!(git(dir, &["diff", "--cached"]), "");
}

#[test]
fn signs_commits_when_configured() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    let key = dir.join("id");
    let keygen = Command::new("ssh-keygen")
        .args(["-q", "-t", "ed25519", "-N", "", "-C", "test", "-f"])
        .arg(&key)
        .output();
    match keygen {
        Ok(out) if out.status.success() => {}
        _ => {
            eprintln!("skipping signs_commits_when_configured: ssh-keygen unavailable");
            return;
        }
    }
    let pub_path = dir.join("id.pub");
    let pub_key = fs::read_to_string(&pub_path).unwrap();
    let allowed = dir.join("allowed_signers");
    fs::write(&allowed, format!("* {}", pub_key.trim())).unwrap();
    git(dir, &["config", "gpg.format", "ssh"]);
    git(
        dir,
        &["config", "user.signingkey", pub_path.to_str().unwrap()],
    );
    git(dir, &["config", "commit.gpgsign", "true"]);
    git(
        dir,
        &[
            "config",
            "gpg.ssh.allowedSignersFile",
            allowed.to_str().unwrap(),
        ],
    );

    fs::write(dir.join("a.txt"), "hello\n").unwrap();
    git(dir, &["add", "-A"]);

    let backend = backend::create("git", dir).unwrap();
    let state = backend.read_state().unwrap();
    let mut assign = empty_assignments(&state.files);
    for li in changed_lines(&state.files[0], 0) {
        assign[0][0][li] = Some(0);
    }
    let plan = build_plan(&state.files, &assign, &state.bases, &[spec("signed")]);
    backend.create_commits(&plan).unwrap();

    assert_eq!(git(dir, &["log", "--format=%G?", "-1"]).trim(), "G");
}
