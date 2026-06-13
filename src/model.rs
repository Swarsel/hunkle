use std::collections::HashMap;

use crate::diff::{FileDiff, FileKind, LineKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitSpec {
    pub message: String,
    pub branch: Option<String>,
}

#[derive(Debug)]
pub struct FileChange {
    pub path: String,
    pub mode: String,
    pub content: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct PlannedCommit {
    pub message: String,
    pub branch: Option<String>,
    pub files: Vec<FileChange>,
}

#[derive(Debug)]
pub struct IndexUpdate {
    pub path: String,
    pub mode: String,
    pub content: Option<Vec<u8>>,
    pub staged: Option<Vec<u8>>,
}

#[derive(Debug)]
pub struct Plan {
    pub commits: Vec<PlannedCommit>,
    pub index_updates: Vec<IndexUpdate>,
}

pub type Assignments = Vec<Vec<Vec<Option<usize>>>>;

pub fn empty_assignments(files: &[FileDiff]) -> Assignments {
    files
        .iter()
        .map(|f| f.hunks.iter().map(|h| vec![None; h.lines.len()]).collect())
        .collect()
}

pub fn apply_file(
    base: &[u8],
    file: &FileDiff,
    assign: &[Vec<Option<usize>>],
    include: impl Fn(usize) -> bool,
) -> Vec<u8> {
    let base_lines: Vec<&[u8]> = split_inclusive_newlines(base);
    let mut out: Vec<u8> = Vec::with_capacity(base.len());
    let mut cursor = 0usize;

    for (h_idx, hunk) in file.hunks.iter().enumerate() {
        let copy_until = if hunk.old_count == 0 {
            hunk.old_start
        } else {
            hunk.old_start - 1
        };
        while cursor < copy_until && cursor < base_lines.len() {
            out.extend_from_slice(base_lines[cursor]);
            cursor += 1;
        }
        for (l_idx, line) in hunk.lines.iter().enumerate() {
            let included = assign[h_idx][l_idx].is_some_and(&include);
            match line.kind {
                LineKind::Context => {
                    if cursor < base_lines.len() {
                        out.extend_from_slice(base_lines[cursor]);
                    }
                    cursor += 1;
                }
                LineKind::Del => {
                    if !included && cursor < base_lines.len() {
                        out.extend_from_slice(base_lines[cursor]);
                    }
                    cursor += 1;
                }
                LineKind::Add => {
                    if included {
                        out.extend_from_slice(line.content.as_bytes());
                        if !line.no_newline {
                            out.push(b'\n');
                        }
                    }
                }
            }
        }
    }
    while cursor < base_lines.len() {
        out.extend_from_slice(base_lines[cursor]);
        cursor += 1;
    }
    out
}

fn split_inclusive_newlines(b: &[u8]) -> Vec<&[u8]> {
    let mut lines = Vec::new();
    let mut start = 0;
    for (i, &c) in b.iter().enumerate() {
        if c == b'\n' {
            lines.push(&b[start..=i]);
            start = i + 1;
        }
    }
    if start < b.len() {
        lines.push(&b[start..]);
    }
    lines
}

pub fn build_plan(
    files: &[FileDiff],
    assign: &Assignments,
    bases: &HashMap<String, Vec<u8>>,
    commits: &[CommitSpec],
) -> Plan {
    let empty = Vec::new();
    let mut planned = Vec::new();
    for (ci, spec) in commits.iter().enumerate() {
        let mut changes = Vec::new();
        let same_dest_upto = |a: usize| a <= ci && commits[a].branch == spec.branch;
        for (fi, file) in files.iter().enumerate() {
            let touched = assign[fi].iter().flatten().any(|a| *a == Some(ci));
            if !touched {
                continue;
            }
            let all_included = file.hunks.iter().enumerate().all(|(hi, h)| {
                h.lines
                    .iter()
                    .enumerate()
                    .all(|(li, l)| !l.is_change() || assign[fi][hi][li].is_some_and(same_dest_upto))
            });
            if file.kind == FileKind::Deleted && all_included {
                changes.push(FileChange {
                    path: file.path.clone(),
                    mode: file.mode.clone(),
                    content: None,
                });
                continue;
            }
            let base = bases.get(&file.path).unwrap_or(&empty);
            let content = apply_file(base, file, &assign[fi], same_dest_upto);
            changes.push(FileChange {
                path: file.path.clone(),
                mode: file.mode.clone(),
                content: Some(content),
            });
        }
        if !changes.is_empty() {
            planned.push(PlannedCommit {
                message: spec.message.clone(),
                branch: spec.branch.clone(),
                files: changes,
            });
        }
    }
    Plan {
        commits: planned,
        index_updates: index_updates(files, assign, bases, commits),
    }
}

const KEEP: usize = usize::MAX;

fn index_updates(
    files: &[FileDiff],
    assign: &Assignments,
    bases: &HashMap<String, Vec<u8>>,
    commits: &[CommitSpec],
) -> Vec<IndexUpdate> {
    let empty = Vec::new();
    let mut out = Vec::new();
    for (fi, file) in files.iter().enumerate() {
        let branched = assign[fi]
            .iter()
            .flatten()
            .any(|a| a.is_some_and(|ci| commits[ci].branch.is_some()));
        if !branched {
            continue;
        }
        let base = bases.get(&file.path).unwrap_or(&empty);
        let marked: Vec<Vec<Option<usize>>> = assign[fi]
            .iter()
            .map(|h| h.iter().map(|a| Some(a.unwrap_or(KEEP))).collect())
            .collect();
        let staged =
            (file.kind != FileKind::Deleted).then(|| apply_file(base, file, &marked, |_| true));
        let kept_change = file.hunks.iter().enumerate().any(|(hi, h)| {
            h.lines.iter().enumerate().any(|(li, l)| {
                l.is_change() && assign[fi][hi][li].is_none_or(|ci| commits[ci].branch.is_none())
            })
        });
        let content = if file.kind == FileKind::Added && !kept_change {
            None
        } else {
            Some(apply_file(base, file, &marked, |a| {
                a == KEEP || commits[a].branch.is_none()
            }))
        };
        out.push(IndexUpdate {
            path: file.path.clone(),
            mode: file.mode.clone(),
            content,
            staged,
        });
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;

    fn modified_file() -> Vec<FileDiff> {
        diff::parse(
            "\
diff --git a/f.txt b/f.txt
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/f.txt
+++ b/f.txt
@@ -1,4 +1,5 @@
 one
-two
+TWO
+two-and-a-half
 three
 four
",
        )
        .unwrap()
    }

    const BASE: &str = "one\ntwo\nthree\nfour\n";

    fn spec(message: &str) -> CommitSpec {
        CommitSpec {
            message: message.to_string(),
            branch: None,
        }
    }

    fn branch_spec(message: &str, branch: &str) -> CommitSpec {
        CommitSpec {
            message: message.to_string(),
            branch: Some(branch.to_string()),
        }
    }

    #[test]
    fn apply_full_hunk() {
        let files = modified_file();
        let mut assign = empty_assignments(&files);
        assign[0][0][1] = Some(0);
        assign[0][0][2] = Some(0);
        assign[0][0][3] = Some(0);
        let out = apply_file(BASE.as_bytes(), &files[0], &assign[0], |a| a == 0);
        assert_eq!(out, b"one\nTWO\ntwo-and-a-half\nthree\nfour\n");
    }

    #[test]
    fn apply_subset_of_lines() {
        let files = modified_file();
        let mut assign = empty_assignments(&files);
        assign[0][0][2] = Some(0);
        let out = apply_file(BASE.as_bytes(), &files[0], &assign[0], |a| a == 0);
        assert_eq!(out, b"one\ntwo\nTWO\nthree\nfour\n");
    }

    #[test]
    fn apply_is_cumulative_across_commits() {
        let files = modified_file();
        let mut assign = empty_assignments(&files);
        assign[0][0][1] = Some(0);
        assign[0][0][2] = Some(0);
        assign[0][0][3] = Some(1);
        let first = apply_file(BASE.as_bytes(), &files[0], &assign[0], |a| a == 0);
        assert_eq!(first, b"one\nTWO\nthree\nfour\n");
        let second = apply_file(BASE.as_bytes(), &files[0], &assign[0], |a| a <= 1);
        assert_eq!(second, b"one\nTWO\ntwo-and-a-half\nthree\nfour\n");
    }

    #[test]
    fn plan_deletes_file_only_when_fully_assigned() {
        let files = diff::parse(
            "\
diff --git a/gone.txt b/gone.txt
deleted file mode 100644
index 1111111111111111111111111111111111111111..0000000000000000000000000000000000000000
--- a/gone.txt
+++ /dev/null
@@ -1,2 +0,0 @@
-bye
-now
",
        )
        .unwrap();
        let mut assign = empty_assignments(&files);
        assign[0][0][0] = Some(0);
        assign[0][0][1] = Some(1);
        let bases = HashMap::from([("gone.txt".to_string(), b"bye\nnow\n".to_vec())]);
        let specs = vec![spec("first"), spec("second")];
        let plan = build_plan(&files, &assign, &bases, &specs);
        assert_eq!(plan.commits.len(), 2);
        assert_eq!(
            plan.commits[0].files[0].content.as_deref(),
            Some(b"now\n".as_slice())
        );
        assert!(plan.commits[1].files[0].content.is_none());
        assert!(plan.index_updates.is_empty());
    }

    #[test]
    fn plan_skips_empty_commits() {
        let files = modified_file();
        let mut assign = empty_assignments(&files);
        assign[0][0][1] = Some(1);
        let bases = HashMap::from([("f.txt".to_string(), BASE.as_bytes().to_vec())]);
        let specs = vec![spec("empty"), spec("used")];
        let plan = build_plan(&files, &assign, &bases, &specs);
        assert_eq!(plan.commits.len(), 1);
        assert_eq!(plan.commits[0].message, "used");
    }

    #[test]
    fn branch_commits_are_independent_and_unstaged() {
        let files = modified_file();
        let mut assign = empty_assignments(&files);
        assign[0][0][1] = Some(0);
        assign[0][0][2] = Some(0);
        assign[0][0][3] = Some(1);
        let bases = HashMap::from([("f.txt".to_string(), BASE.as_bytes().to_vec())]);
        let specs = vec![spec("here"), branch_spec("side", "topic")];
        let plan = build_plan(&files, &assign, &bases, &specs);

        assert_eq!(plan.commits.len(), 2);
        assert_eq!(plan.commits[1].branch.as_deref(), Some("topic"));
        assert_eq!(
            plan.commits[0].files[0].content.as_deref(),
            Some(b"one\nTWO\nthree\nfour\n".as_slice())
        );
        assert_eq!(
            plan.commits[1].files[0].content.as_deref(),
            Some(b"one\ntwo\ntwo-and-a-half\nthree\nfour\n".as_slice())
        );

        assert_eq!(plan.index_updates.len(), 1);
        let up = &plan.index_updates[0];
        assert_eq!(
            up.content.as_deref(),
            Some(b"one\nTWO\nthree\nfour\n".as_slice())
        );
        assert_eq!(
            up.staged.as_deref(),
            Some(b"one\nTWO\ntwo-and-a-half\nthree\nfour\n".as_slice())
        );
    }

    #[test]
    fn added_file_fully_branched_is_removed_from_index() {
        let files = diff::parse(
            "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
",
        )
        .unwrap();
        let mut assign = empty_assignments(&files);
        assign[0][0][0] = Some(0);
        assign[0][0][1] = Some(0);
        let plan = build_plan(
            &files,
            &assign,
            &HashMap::new(),
            &[branch_spec("side", "topic")],
        );
        assert_eq!(plan.commits.len(), 1);
        assert_eq!(plan.index_updates.len(), 1);
        assert!(plan.index_updates[0].content.is_none());
        assert_eq!(
            plan.index_updates[0].staged.as_deref(),
            Some(b"hello\nworld\n".as_slice())
        );
    }

    #[test]
    fn apply_new_file() {
        let files = diff::parse(
            "\
diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
",
        )
        .unwrap();
        let mut assign = empty_assignments(&files);
        assign[0][0][0] = Some(0);
        let out = apply_file(b"", &files[0], &assign[0], |a| a == 0);
        assert_eq!(out, b"hello\n");
    }
}
