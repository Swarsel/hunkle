use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

use crate::diff::{self, FileDiff, FileKind};
use crate::model::{IndexUpdate, Plan};

pub struct RepoState {
    pub files: Vec<FileDiff>,
    pub bases: HashMap<String, Vec<u8>>,
    pub branch: String,
}

#[derive(Debug)]
pub struct CreatedCommits {
    pub ids: Vec<String>,
    pub worktree_skipped: Vec<String>,
}

pub trait Backend {
    fn read_state(&self) -> Result<RepoState>;
    fn create_commits(&self, plan: &Plan) -> Result<CreatedCommits>;
}

pub fn create(name: &str, dir: &Path) -> Result<Box<dyn Backend>> {
    match name {
        "git" => Ok(Box::new(GitBackend {
            dir: dir.to_path_buf(),
        })),
        other => bail!("unknown backend {other:?} (available: git)"),
    }
}

pub struct GitBackend {
    dir: PathBuf,
}

impl GitBackend {
    fn git(&self, args: &[&str], stdin: Option<&[u8]>, index: Option<&Path>) -> Result<Vec<u8>> {
        let mut cmd = Command::new("git");
        cmd.arg("-C").arg(&self.dir).args(args);
        if let Some(idx) = index {
            cmd.env("GIT_INDEX_FILE", idx);
        }
        cmd.stdin(if stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        });
        cmd.stdout(Stdio::piped());
        cmd.stderr(Stdio::piped());
        let mut child = cmd
            .spawn()
            .with_context(|| format!("failed to run git {}", args.join(" ")))?;
        if let Some(data) = stdin {
            child
                .stdin
                .take()
                .context("no stdin handle")?
                .write_all(data)?;
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!(
                "git {} failed: {}",
                args.join(" "),
                String::from_utf8_lossy(&out.stderr).trim()
            );
        }
        Ok(out.stdout)
    }

    fn git_str(&self, args: &[&str], index: Option<&Path>) -> Result<String> {
        Ok(String::from_utf8_lossy(&self.git(args, None, index)?)
            .trim_end()
            .to_string())
    }

    fn gpg_sign(&self) -> bool {
        self.git_str(&["config", "--bool", "--get", "commit.gpgsign"], None)
            .map(|s| s == "true")
            .unwrap_or(false)
    }

    fn head(&self) -> Option<String> {
        self.git_str(&["rev-parse", "--verify", "--quiet", "HEAD"], None)
            .ok()
            .filter(|s| !s.is_empty())
    }
}

impl Backend for GitBackend {
    fn read_state(&self) -> Result<RepoState> {
        self.git_str(&["rev-parse", "--git-dir"], None)
            .context("not a git repository")?;
        let diff_text = String::from_utf8_lossy(&self.git(
            &[
                "-c",
                "core.quotePath=false",
                "diff",
                "--cached",
                "--no-color",
                "--no-ext-diff",
                "--no-renames",
                "--full-index",
                "-U3",
            ],
            None,
            None,
        )?)
        .into_owned();
        let files = diff::parse(&diff_text)?;
        let mut bases = HashMap::new();
        for f in &files {
            if f.binary || f.kind == FileKind::Added {
                continue;
            }
            if let Some(blob) = &f.old_blob {
                let content = self.git(&["cat-file", "blob", blob], None, None)?;
                bases.insert(f.path.clone(), content);
            }
        }
        let branch = self
            .git_str(&["rev-parse", "--abbrev-ref", "HEAD"], None)
            .unwrap_or_else(|_| "HEAD".to_string());
        Ok(RepoState {
            files,
            bases,
            branch,
        })
    }

    fn create_commits(&self, plan: &Plan) -> Result<CreatedCommits> {
        if plan.commits.is_empty() {
            bail!("nothing to commit");
        }
        let mut branches: Vec<String> = Vec::new();
        for commit in &plan.commits {
            if let Some(b) = &commit.branch
                && !branches.contains(b)
            {
                branches.push(b.clone());
            }
        }
        for b in &branches {
            self.git(&["check-ref-format", "--branch", b], None, None)
                .map_err(|_| anyhow::anyhow!("invalid branch name {b:?}"))?;
            let ref_name = format!("refs/heads/{b}");
            if self
                .git_str(&["rev-parse", "--verify", "--quiet", &ref_name], None)
                .is_ok_and(|s| !s.is_empty())
            {
                bail!("branch {b:?} already exists");
            }
        }
        let git_dir = self.git_str(&["rev-parse", "--absolute-git-dir"], None)?;
        let original_head = self.head();
        let sign = self.gpg_sign();
        let mut tmp_indexes: Vec<PathBuf> = Vec::new();

        let result = (|| {
            let mut dests: HashMap<Option<String>, (PathBuf, Option<String>)> = HashMap::new();
            let mut created = Vec::new();
            for commit in &plan.commits {
                if !dests.contains_key(&commit.branch) {
                    let path = PathBuf::from(&git_dir).join(format!(
                        "hunkle-index-{}-{}",
                        std::process::id(),
                        dests.len()
                    ));
                    let idx = Some(path.as_path());
                    match &original_head {
                        Some(head) => self.git(&["read-tree", head], None, idx)?,
                        None => self.git(&["read-tree", "--empty"], None, idx)?,
                    };
                    tmp_indexes.push(path.clone());
                    dests.insert(commit.branch.clone(), (path, original_head.clone()));
                }
                let (path, parent) = dests.get_mut(&commit.branch).expect("dest just inserted");
                let idx = Some(path.as_path());
                for fc in &commit.files {
                    match &fc.content {
                        Some(content) => {
                            let sha = String::from_utf8_lossy(&self.git(
                                &["hash-object", "-w", "--stdin"],
                                Some(content),
                                None,
                            )?)
                            .trim()
                            .to_string();
                            let cacheinfo = format!("{},{},{}", fc.mode, sha, fc.path);
                            self.git(
                                &["update-index", "--add", "--cacheinfo", &cacheinfo],
                                None,
                                idx,
                            )?;
                        }
                        None => {
                            self.git(
                                &["update-index", "--force-remove", "--", &fc.path],
                                None,
                                idx,
                            )?;
                        }
                    }
                }
                let tree = self.git_str(&["write-tree"], idx)?;
                let mut args = vec!["commit-tree", tree.as_str()];
                if sign {
                    args.push("-S");
                }
                if let Some(p) = parent.as_deref() {
                    args.push("-p");
                    args.push(p);
                }
                args.push("-m");
                args.push(&commit.message);
                let sha = self.git_str(&args, None)?;
                *parent = Some(sha.clone());
                created.push(sha);
            }
            if let Some((_, Some(tip))) = dests.get(&None) {
                match &original_head {
                    Some(head) => self.git(
                        &[
                            "update-ref",
                            "-m",
                            "hunkle: split staged changes",
                            "HEAD",
                            tip,
                            head,
                        ],
                        None,
                        None,
                    )?,
                    None => self.git(&["update-ref", "HEAD", tip], None, None)?,
                };
            }
            for b in &branches {
                if let Some((_, Some(tip))) = dests.get(&Some(b.clone())) {
                    self.git(&["branch", b, tip], None, None)?;
                }
            }
            let worktree_skipped = self.apply_index_updates(&plan.index_updates)?;
            Ok(CreatedCommits {
                ids: created,
                worktree_skipped,
            })
        })();

        for path in &tmp_indexes {
            let _ = std::fs::remove_file(path);
        }
        result
    }
}

impl GitBackend {
    fn apply_index_updates(&self, updates: &[IndexUpdate]) -> Result<Vec<String>> {
        let mut skipped = Vec::new();
        for up in updates {
            match &up.content {
                Some(content) => {
                    let sha = String::from_utf8_lossy(&self.git(
                        &["hash-object", "-w", "--stdin"],
                        Some(content),
                        None,
                    )?)
                    .trim()
                    .to_string();
                    let cacheinfo = format!("{},{},{}", up.mode, sha, up.path);
                    self.git(
                        &["update-index", "--add", "--cacheinfo", &cacheinfo],
                        None,
                        None,
                    )?;
                }
                None => {
                    self.git(
                        &["update-index", "--force-remove", "--", &up.path],
                        None,
                        None,
                    )?;
                }
            }
            let wt_path = self.dir.join(&up.path);
            let wt = std::fs::read(&wt_path).ok();
            if wt.as_deref() == up.staged.as_deref() {
                match &up.content {
                    Some(content) => {
                        if let Some(dir) = wt_path.parent() {
                            std::fs::create_dir_all(dir)?;
                        }
                        std::fs::write(&wt_path, content)?;
                    }
                    None => {
                        let _ = std::fs::remove_file(&wt_path);
                    }
                }
            } else {
                skipped.push(up.path.clone());
            }
        }
        Ok(skipped)
    }
}
