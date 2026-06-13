use anyhow::{Context as _, Result, bail};
use serde::{Deserialize, Serialize};

use crate::backend::{Backend, RepoState};
use crate::diff::{FileDiff, FileKind, LineKind};
use crate::model::{CommitSpec, build_plan, empty_assignments};

pub const PROTOCOL_VERSION: u32 = 2;

#[derive(Serialize)]
struct DumpOut<'a> {
    version: u32,
    branch: &'a str,
    token: String,
    files: Vec<FileOut<'a>>,
}

#[derive(Serialize)]
struct FileOut<'a> {
    path: &'a str,
    kind: &'static str,
    binary: bool,
    hunks: Vec<HunkOut<'a>>,
}

#[derive(Serialize)]
struct HunkOut<'a> {
    header: &'a str,
    lines: Vec<LineOut<'a>>,
}

#[derive(Serialize)]
struct LineOut<'a> {
    kind: &'static str,
    content: &'a str,
}

#[derive(Deserialize)]
struct ApplyIn {
    token: String,
    commits: Vec<CommitIn>,
    assignments: Vec<(usize, usize, usize, usize)>,
}

#[derive(Deserialize)]
#[serde(untagged)]
enum CommitIn {
    Message(String),
    Spec {
        message: String,
        #[serde(default)]
        branch: Option<String>,
    },
}

#[derive(Serialize)]
struct ApplyOut {
    commits: Vec<CommitOut>,
    worktree_skipped: Vec<String>,
}

#[derive(Serialize)]
struct CommitOut {
    id: String,
    message: String,
    branch: Option<String>,
}

fn files_out(files: &[FileDiff]) -> Vec<FileOut<'_>> {
    files
        .iter()
        .map(|f| FileOut {
            path: &f.path,
            kind: match f.kind {
                FileKind::Modified => "modified",
                FileKind::Added => "added",
                FileKind::Deleted => "deleted",
            },
            binary: f.binary,
            hunks: f
                .hunks
                .iter()
                .map(|h| HunkOut {
                    header: &h.header,
                    lines: h
                        .lines
                        .iter()
                        .map(|l| LineOut {
                            kind: match l.kind {
                                LineKind::Context => "context",
                                LineKind::Add => "add",
                                LineKind::Del => "del",
                            },
                            content: &l.content,
                        })
                        .collect(),
                })
                .collect(),
        })
        .collect()
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in data {
        h ^= b as u64;
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    h
}

fn state_token(state: &RepoState) -> Result<String> {
    let json = serde_json::to_string(&files_out(&state.files))?;
    Ok(format!("{:016x}", fnv1a(json.as_bytes())))
}

pub fn dump(backend: &dyn Backend) -> Result<String> {
    let state = backend.read_state()?;
    let out = DumpOut {
        version: PROTOCOL_VERSION,
        branch: &state.branch,
        token: state_token(&state)?,
        files: files_out(&state.files),
    };
    Ok(serde_json::to_string_pretty(&out)?)
}

pub fn apply(backend: &dyn Backend, input: &str) -> Result<String> {
    let req: ApplyIn = serde_json::from_str(input).context("invalid plan JSON")?;
    let state = backend.read_state()?;
    if state_token(&state)? != req.token {
        bail!("staged changes have changed since the plan was created; re-run dump");
    }
    let specs: Vec<CommitSpec> = req
        .commits
        .into_iter()
        .map(|c| match c {
            CommitIn::Message(message) => CommitSpec {
                message,
                branch: None,
            },
            CommitIn::Spec { message, branch } => CommitSpec { message, branch },
        })
        .collect();
    if specs.iter().any(|c| c.message.trim().is_empty()) {
        bail!("commit messages cannot be empty");
    }
    if specs
        .iter()
        .any(|c| c.branch.as_deref().is_some_and(|b| b.trim().is_empty()))
    {
        bail!("branch names cannot be empty");
    }
    let mut assign = empty_assignments(&state.files);
    for &(fi, hi, li, ci) in &req.assignments {
        let line = state
            .files
            .get(fi)
            .and_then(|f| f.hunks.get(hi))
            .and_then(|h| h.lines.get(li))
            .with_context(|| format!("assignment ({fi},{hi},{li}) out of range"))?;
        if !line.is_change() {
            bail!("assignment ({fi},{hi},{li}) targets a context line");
        }
        if ci >= specs.len() {
            bail!("assignment ({fi},{hi},{li}) targets unknown commit {ci}");
        }
        assign[fi][hi][li] = Some(ci);
    }
    let plan = build_plan(&state.files, &assign, &state.bases, &specs);
    if plan.commits.is_empty() {
        bail!("no lines assigned; nothing to commit");
    }
    let created = backend.create_commits(&plan)?;
    let out = ApplyOut {
        commits: created
            .ids
            .into_iter()
            .zip(plan.commits.iter())
            .map(|(id, c)| CommitOut {
                id,
                message: c.message.clone(),
                branch: c.branch.clone(),
            })
            .collect(),
        worktree_skipped: created.worktree_skipped,
    };
    Ok(serde_json::to_string_pretty(&out)?)
}
