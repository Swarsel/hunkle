use std::collections::HashMap;
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

use anyhow::{Context as _, Result, bail};

use crate::diff::{self, FileDiff, FileKind};
use crate::model::PlannedCommit;

pub struct RepoState {
    pub files: Vec<FileDiff>,
    pub bases: HashMap<String, Vec<u8>>,
    pub branch: String,
}

pub trait Backend {
    fn read_state(&self) -> Result<RepoState>;
    fn create_commits(&self, plan: &[PlannedCommit]) -> Result<Vec<String>>;
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

    fn create_commits(&self, plan: &[PlannedCommit]) -> Result<Vec<String>> {
        if plan.is_empty() {
            bail!("nothing to commit");
        }
        let git_dir = self.git_str(&["rev-parse", "--absolute-git-dir"], None)?;
        let tmp_index = PathBuf::from(git_dir).join(format!("hunkle-index-{}", std::process::id()));
        let original_head = self.head();

        let result = (|| {
            let idx = Some(tmp_index.as_path());
            match &original_head {
                Some(head) => self.git(&["read-tree", head], None, idx)?,
                None => self.git(&["read-tree", "--empty"], None, idx)?,
            };
            let mut parent = original_head.clone();
            let mut created = Vec::new();
            for commit in plan {
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
                if let Some(p) = &parent {
                    args.push("-p");
                    args.push(p.as_str());
                }
                args.push("-m");
                args.push(&commit.message);
                let sha = self.git_str(&args, None)?;
                parent = Some(sha.clone());
                created.push(sha);
            }
            let last = created.last().expect("plan is non-empty");
            match &original_head {
                Some(head) => self.git(
                    &[
                        "update-ref",
                        "-m",
                        "hunkle: split staged changes",
                        "HEAD",
                        last,
                        head,
                    ],
                    None,
                    None,
                )?,
                None => self.git(&["update-ref", "HEAD", last], None, None)?,
            };
            Ok(created)
        })();

        let _ = std::fs::remove_file(&tmp_index);
        result
    }
}
