use anyhow::{Context as _, Result, bail};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineKind {
    Context,
    Add,
    Del,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub kind: LineKind,
    pub content: String,
    pub no_newline: bool,
}

impl DiffLine {
    pub fn is_change(&self) -> bool {
        self.kind != LineKind::Context
    }
}

#[derive(Debug, Clone)]
pub struct Hunk {
    pub old_start: usize,
    pub old_count: usize,
    pub new_start: usize,
    pub new_count: usize,
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileKind {
    Modified,
    Added,
    Deleted,
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub path: String,
    pub kind: FileKind,
    pub mode: String,
    pub old_blob: Option<String>,
    pub binary: bool,
    pub hunks: Vec<Hunk>,
}

pub fn parse(diff: &str) -> Result<Vec<FileDiff>> {
    let mut files: Vec<FileDiff> = Vec::new();
    let mut lines = diff.lines().peekable();

    while let Some(line) = lines.next() {
        if let Some(rest) = line.strip_prefix("diff --git ") {
            files.push(FileDiff {
                path: path_from_git_line(rest),
                kind: FileKind::Modified,
                mode: "100644".to_string(),
                old_blob: None,
                binary: false,
                hunks: Vec::new(),
            });
            continue;
        }
        let file = match files.last_mut() {
            Some(f) => f,
            None => continue,
        };
        if let Some(mode) = line.strip_prefix("new file mode ") {
            file.kind = FileKind::Added;
            file.mode = mode.to_string();
        } else if let Some(mode) = line.strip_prefix("deleted file mode ") {
            file.kind = FileKind::Deleted;
            file.mode = mode.to_string();
        } else if let Some(mode) = line.strip_prefix("new mode ") {
            file.mode = mode.to_string();
        } else if let Some(rest) = line.strip_prefix("index ") {
            if let Some((old, tail)) = rest.split_once("..") {
                if !old.chars().all(|c| c == '0') {
                    file.old_blob = Some(old.to_string());
                }
                if let Some((_, mode)) = tail.split_once(' ') {
                    file.mode = mode.to_string();
                }
            }
        } else if let Some(p) = line.strip_prefix("--- ") {
            if p != "/dev/null" {
                file.path = strip_prefix_dir(p);
            }
        } else if let Some(p) = line.strip_prefix("+++ ") {
            if p != "/dev/null" {
                file.path = strip_prefix_dir(p);
            }
        } else if line.starts_with("Binary files ") || line == "GIT binary patch" {
            file.binary = true;
        } else if line.starts_with("@@ ") {
            let mut hunk =
                parse_hunk_header(line).with_context(|| format!("bad hunk header: {line}"))?;
            let mut old_left = hunk.old_count;
            let mut new_left = hunk.new_count;
            while old_left > 0 || new_left > 0 {
                let l = lines.next().context("diff ended in the middle of a hunk")?;
                let (kind, content) = match l.chars().next() {
                    Some('+') => (LineKind::Add, &l[1..]),
                    Some('-') => (LineKind::Del, &l[1..]),
                    Some(' ') => (LineKind::Context, &l[1..]),
                    None => (LineKind::Context, ""),
                    Some('\\') => {
                        if let Some(last) = hunk.lines.last_mut() {
                            last.no_newline = true;
                        }
                        continue;
                    }
                    Some(c) => bail!("unexpected line in hunk (starts with {c:?}): {l}"),
                };
                match kind {
                    LineKind::Context => {
                        old_left = old_left.saturating_sub(1);
                        new_left = new_left.saturating_sub(1);
                    }
                    LineKind::Del => old_left = old_left.saturating_sub(1),
                    LineKind::Add => new_left = new_left.saturating_sub(1),
                }
                hunk.lines.push(DiffLine {
                    kind,
                    content: content.to_string(),
                    no_newline: false,
                });
            }
            if lines.peek().is_some_and(|l| l.starts_with('\\')) {
                lines.next();
                if let Some(last) = hunk.lines.last_mut() {
                    last.no_newline = true;
                }
            }
            file.hunks.push(hunk);
        }
    }
    Ok(files)
}

fn path_from_git_line(rest: &str) -> String {
    if let Some((_, b)) = rest.split_once(" b/") {
        return b.to_string();
    }
    rest.to_string()
}

fn strip_prefix_dir(p: &str) -> String {
    p.strip_prefix("a/")
        .or_else(|| p.strip_prefix("b/"))
        .unwrap_or(p)
        .to_string()
}

fn parse_hunk_header(line: &str) -> Result<Hunk> {
    let inner = line
        .strip_prefix("@@ ")
        .and_then(|r| r.split_once(" @@"))
        .map(|(ranges, _)| ranges)
        .context("not a hunk header")?;
    let (old, new) = inner.split_once(' ').context("missing range")?;
    let (old_start, old_count) = parse_range(old.strip_prefix('-').context("bad old range")?)?;
    let (new_start, new_count) = parse_range(new.strip_prefix('+').context("bad new range")?)?;
    Ok(Hunk {
        old_start,
        old_count,
        new_start,
        new_count,
        header: line.to_string(),
        lines: Vec::new(),
    })
}

fn parse_range(r: &str) -> Result<(usize, usize)> {
    Ok(match r.split_once(',') {
        Some((s, c)) => (s.parse()?, c.parse()?),
        None => (r.parse()?, 1),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = "\
diff --git a/src/lib.rs b/src/lib.rs
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/src/lib.rs
+++ b/src/lib.rs
@@ -1,4 +1,5 @@
 fn main() {
-    println!(\"old\");
+    println!(\"new\");
+    println!(\"extra\");
 }
 // end
@@ -10,2 +11,2 @@ fn helper() {
-    let a = 1;
+    let a = 2;

diff --git a/new.txt b/new.txt
new file mode 100644
index 0000000000000000000000000000000000000000..3333333333333333333333333333333333333333
--- /dev/null
+++ b/new.txt
@@ -0,0 +1,2 @@
+hello
+world
\\ No newline at end of file
diff --git a/gone.txt b/gone.txt
deleted file mode 100755
index 4444444444444444444444444444444444444444..0000000000000000000000000000000000000000
--- a/gone.txt
+++ /dev/null
@@ -1,1 +0,0 @@
-bye
diff --git a/img.png b/img.png
index 5555555555555555555555555555555555555555..6666666666666666666666666666666666666666 100644
Binary files a/img.png and b/img.png differ
";

    #[test]
    fn parses_files_and_hunks() {
        let files = parse(SAMPLE).unwrap();
        assert_eq!(files.len(), 4);

        let f = &files[0];
        assert_eq!(f.path, "src/lib.rs");
        assert_eq!(f.kind, FileKind::Modified);
        assert_eq!(
            f.old_blob.as_deref(),
            Some("1111111111111111111111111111111111111111")
        );
        assert_eq!(f.hunks.len(), 2);
        assert_eq!(f.hunks[0].old_start, 1);
        assert_eq!(f.hunks[0].lines.len(), 6);
        assert_eq!(f.hunks[0].lines[1].kind, LineKind::Del);
        assert_eq!(f.hunks[0].lines[2].kind, LineKind::Add);
        assert_eq!(f.hunks[1].lines.last().unwrap().kind, LineKind::Context);
        assert_eq!(f.hunks[1].lines.last().unwrap().content, "");

        let f = &files[1];
        assert_eq!(f.kind, FileKind::Added);
        assert!(f.old_blob.is_none());
        assert!(f.hunks[0].lines[1].no_newline);

        let f = &files[2];
        assert_eq!(f.kind, FileKind::Deleted);
        assert_eq!(f.mode, "100755");

        let f = &files[3];
        assert!(f.binary);
        assert!(f.hunks.is_empty());
    }
}
