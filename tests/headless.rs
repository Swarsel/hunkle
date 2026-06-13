use std::fs;
use std::io::Write as _;
use std::path::Path;
use std::process::{Command, Stdio};

use serde_json::{Value, json};

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

fn hunkle(dir: &Path, args: &[&str], stdin: Option<&str>) -> Result<String, String> {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_hunkle"));
    cmd.arg("-C").arg(dir).args(args);
    cmd.stdin(if stdin.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("failed to run hunkle");
    if let Some(data) = stdin {
        child
            .stdin
            .take()
            .unwrap()
            .write_all(data.as_bytes())
            .unwrap();
    }
    let out = child.wait_with_output().unwrap();
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

fn setup_repo(dir: &Path) {
    git(dir, &["init", "-b", "main"]);
    git(dir, &["config", "user.name", "test"]);
    git(dir, &["config", "user.email", "test@example.com"]);
    git(dir, &["config", "commit.gpgsign", "false"]);
}

#[test]
fn dump_and_apply_roundtrip() {
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
    git(dir, &["add", "-A"]);

    let dump: Value = serde_json::from_str(&hunkle(dir, &["dump"], None).unwrap()).unwrap();
    assert_eq!(dump["version"], 2);
    assert_eq!(dump["branch"], "main");
    let files = dump["files"].as_array().unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0]["path"], "f.txt");
    let hunks = files[0]["hunks"].as_array().unwrap();
    assert_eq!(hunks.len(), 2);
    assert_eq!(hunks[0]["lines"][1]["kind"], "del");
    assert_eq!(hunks[0]["lines"][2]["kind"], "add");

    let changed = |h: usize| -> Vec<usize> {
        hunks[h]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, l)| l["kind"] != "context")
            .map(|(i, _)| i)
            .collect()
    };
    let mut assignments = vec![];
    for li in changed(0) {
        assignments.push(json!([0, 0, li, 0]));
    }
    for li in changed(1) {
        assignments.push(json!([0, 1, li, 1]));
    }
    let plan = json!({
        "token": dump["token"],
        "commits": ["first", "second"],
        "assignments": assignments,
    })
    .to_string();

    let result: Value =
        serde_json::from_str(&hunkle(dir, &["apply"], Some(&plan)).unwrap()).unwrap();
    let commits = result["commits"].as_array().unwrap();
    assert_eq!(commits.len(), 2);
    assert_eq!(commits[0]["message"], "first");

    let log = git(dir, &["log", "--format=%s"]);
    assert_eq!(
        log.trim().lines().collect::<Vec<_>>(),
        vec!["second", "first", "base"]
    );
    assert_eq!(git(dir, &["diff", "--cached"]), "");
    assert_eq!(git(dir, &["diff"]), "");
}

#[test]
fn apply_creates_branch_commits() {
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
    git(dir, &["add", "-A"]);

    let dump: Value = serde_json::from_str(&hunkle(dir, &["dump"], None).unwrap()).unwrap();
    let hunks = dump["files"][0]["hunks"].as_array().unwrap();
    let changed = |h: usize| -> Vec<usize> {
        hunks[h]["lines"]
            .as_array()
            .unwrap()
            .iter()
            .enumerate()
            .filter(|(_, l)| l["kind"] != "context")
            .map(|(i, _)| i)
            .collect()
    };
    let mut assignments = vec![];
    for li in changed(0) {
        assignments.push(json!([0, 0, li, 0]));
    }
    for li in changed(1) {
        assignments.push(json!([0, 1, li, 1]));
    }
    let plan = json!({
        "token": dump["token"],
        "commits": ["local", {"message": "side", "branch": "topic"}],
        "assignments": assignments,
    })
    .to_string();

    let result: Value =
        serde_json::from_str(&hunkle(dir, &["apply"], Some(&plan)).unwrap()).unwrap();
    let commits = result["commits"].as_array().unwrap();
    assert_eq!(commits[0]["branch"], Value::Null);
    assert_eq!(commits[1]["branch"], "topic");
    assert_eq!(result["worktree_skipped"].as_array().unwrap().len(), 0);

    assert_eq!(git(dir, &["log", "--format=%s"]).trim(), "local\nbase");
    assert_eq!(
        git(dir, &["log", "--format=%s", "topic"]).trim(),
        "side\nbase"
    );
    assert_eq!(git(dir, &["diff", "--cached"]), "");
    assert_eq!(git(dir, &["diff"]), "");
}

#[test]
fn apply_rejects_stale_token() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    fs::write(dir.join("a.txt"), "one\n").unwrap();
    git(dir, &["add", "-A"]);
    let dump: Value = serde_json::from_str(&hunkle(dir, &["dump"], None).unwrap()).unwrap();

    fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);

    let plan = json!({
        "token": dump["token"],
        "commits": ["initial"],
        "assignments": [[0, 0, 0, 0]],
    })
    .to_string();
    let err = hunkle(dir, &["apply"], Some(&plan)).unwrap_err();
    assert!(err.contains("changed since"), "unexpected error: {err}");
}

#[test]
fn apply_rejects_context_line_assignment() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    setup_repo(dir);

    fs::write(dir.join("a.txt"), "one\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);
    git(dir, &["commit", "-m", "base"]);
    fs::write(dir.join("a.txt"), "ONE\ntwo\n").unwrap();
    git(dir, &["add", "-A"]);

    let dump: Value = serde_json::from_str(&hunkle(dir, &["dump"], None).unwrap()).unwrap();
    let lines = dump["files"][0]["hunks"][0]["lines"].as_array().unwrap();
    let ctx = lines.iter().position(|l| l["kind"] == "context").unwrap();
    let plan = json!({
        "token": dump["token"],
        "commits": ["x"],
        "assignments": [[0, 0, ctx, 0]],
    })
    .to_string();
    let err = hunkle(dir, &["apply"], Some(&plan)).unwrap_err();
    assert!(err.contains("context line"), "unexpected error: {err}");
}
