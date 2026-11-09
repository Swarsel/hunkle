use std::cmp::Reverse;
use std::collections::HashMap;

use crate::diff::{DiffLine, FileDiff, Hunk, LineKind};

pub type Loc = (usize, usize, usize);
pub type Links = HashMap<Loc, Vec<Loc>>;

pub fn prepare(files: &mut Vec<FileDiff>, path: Option<&str>) -> Links {
    let Some(path) = path.map(|p| p.strip_prefix("./").unwrap_or(p)) else {
        return Links::new();
    };
    if let Some(i) = files.iter().position(|f| f.path == path) {
        let f = files.remove(i);
        files.push(f);
    }
    links(files, path)
}

fn key(line: &DiffLine) -> (LineKind, &str) {
    (line.kind, line.content.trim())
}

fn is_blank_change(line: &DiffLine) -> bool {
    line.is_change() && line.content.trim().is_empty()
}

fn significant(hunk: &Hunk) -> Vec<usize> {
    hunk.lines
        .iter()
        .enumerate()
        .filter(|(_, l)| l.is_change() && !l.content.trim().is_empty())
        .map(|(li, _)| li)
        .collect()
}

pub fn links(files: &[FileDiff], path: &str) -> Links {
    let mut out = Links::new();
    let Some(mi) = files.iter().position(|f| f.path == path && !f.binary) else {
        return out;
    };
    let mirror = &files[mi];
    let mirror_sig: Vec<Vec<usize>> = mirror.hunks.iter().map(significant).collect();

    let mut sources: Vec<(usize, usize, Vec<usize>)> = Vec::new();
    for (fi, f) in files.iter().enumerate() {
        if fi == mi || f.binary {
            continue;
        }
        for (hi, h) in f.hunks.iter().enumerate() {
            let sig = significant(h);
            if !sig.is_empty() {
                sources.push((fi, hi, sig));
            }
        }
    }
    sources.sort_by_key(|s| Reverse(s.2.len()));

    let mut claimed: HashMap<(usize, usize), Vec<usize>> = HashMap::new();
    for (fi, hi, sig) in sources {
        let src = &files[fi].hunks[hi].lines;
        let free = |claimed: &HashMap<(usize, usize), Vec<usize>>, mhi, m, strict: bool| {
            claimed
                .get(&(mhi, m))
                .is_none_or(|c| !strict && !c.contains(&fi))
        };
        let found = [true, false].into_iter().find_map(|strict| {
            mirror_sig.iter().enumerate().find_map(|(mhi, msig)| {
                let lines = &mirror.hunks[mhi].lines;
                msig.windows(sig.len())
                    .find(|w| {
                        w.iter().zip(&sig).all(|(&m, &s)| {
                            free(&claimed, mhi, m, strict) && key(&lines[m]) == key(&src[s])
                        })
                    })
                    .map(|w| (mhi, w.to_vec()))
            })
        });
        let Some((mhi, window)) = found else {
            continue;
        };
        let lines = &mirror.hunks[mhi].lines;
        let blank_free = |claimed: &HashMap<_, _>, m: usize| {
            is_blank_change(&lines[m]) && free(claimed, mhi, m, false)
        };

        let mut start = window[0];
        while start > 0 && blank_free(&claimed, start - 1) {
            start -= 1;
        }
        let mut end = *window.last().expect("windows are non-empty");
        while end + 1 < lines.len() && blank_free(&claimed, end + 1) {
            end += 1;
        }
        let mut k = 0;
        for m in start..=end {
            let s = if window.get(k) == Some(&m) {
                k += 1;
                sig[k - 1]
            } else if blank_free(&claimed, m) {
                sig[k.min(sig.len() - 1)]
            } else {
                continue;
            };
            claimed.entry((mhi, m)).or_default().push(fi);
            out.entry((fi, hi, s)).or_default().push((mi, mhi, m));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;

    const DIFF: &str = "\
diff --git a/a.nix b/a.nix
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/a.nix
+++ b/a.nix
@@ -1,3 +1,4 @@
 {
-  foo = 1;
+  foo = 2;
+  bar = 3;
 }
diff --git a/lit.org b/lit.org
index 3333333333333333333333333333333333333333..4444444444444444444444444444444444444444 100644
--- a/lit.org
+++ b/lit.org
@@ -1,6 +1,8 @@
 * heading
 #+begin_src nix
     {
-      foo = 1;
+
+      foo = 2;
+      bar = 3;
     }
 #+end_src
@@ -20 +22 @@
-unrelated
+changed
diff --git a/b.txt b/b.txt
index 5555555555555555555555555555555555555555..6666666666666666666666666666666666666666 100644
--- a/b.txt
+++ b/b.txt
@@ -1 +1 @@
-nope
+not mirrored
";

    #[test]
    fn mirror_file_moves_last_and_links_ignore_indentation() {
        let mut files = diff::parse(DIFF).unwrap();
        let links = prepare(&mut files, Some("./lit.org"));
        let paths: Vec<&str> = files.iter().map(|f| f.path.as_str()).collect();
        assert_eq!(paths, vec!["a.nix", "b.txt", "lit.org"]);

        assert_eq!(links.get(&(0, 0, 1)), Some(&vec![(2, 0, 3)]));
        assert_eq!(links.get(&(0, 0, 2)), Some(&vec![(2, 0, 4), (2, 0, 5)]));
        assert_eq!(links.get(&(0, 0, 3)), Some(&vec![(2, 0, 6)]));
        assert!(!links.keys().any(|&(fi, _, _)| fi == 1));
        assert_eq!(links.len(), 3);
    }

    #[test]
    fn short_hunks_do_not_steal_from_long_ones() {
        let text = "\
diff --git a/a b/a
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/a
+++ b/a
@@ -1 +1 @@
-}
+};
diff --git a/b b/b
index 3333333333333333333333333333333333333333..4444444444444444444444444444444444444444 100644
--- a/b
+++ b/b
@@ -1,2 +1,2 @@
-x
-}
+};
+y
diff --git a/m b/m
index 5555555555555555555555555555555555555555..6666666666666666666666666666666666666666 100644
--- a/m
+++ b/m
@@ -1,3 +1,3 @@
-x
-}
+};
+y
 gap
@@ -10 +10 @@
-}
+};
";
        let files = diff::parse(text).unwrap();
        let links = links(&files, "m");
        assert_eq!(links.get(&(1, 0, 0)), Some(&vec![(2, 0, 0)]));
        assert_eq!(links.get(&(1, 0, 3)), Some(&vec![(2, 0, 3)]));
        assert_eq!(links.get(&(0, 0, 0)), Some(&vec![(2, 1, 0)]));
    }

    #[test]
    fn files_with_the_same_change_share_mirror_lines() {
        let text = "\
diff --git a/gen.el b/gen.el
index 1111111111111111111111111111111111111111..2222222222222222222222222222222222222222 100644
--- a/gen.el
+++ b/gen.el
@@ -1 +1 @@
-(old)
+(new)
diff --git a/lit.org b/lit.org
index 3333333333333333333333333333333333333333..4444444444444444444444444444444444444444 100644
--- a/lit.org
+++ b/lit.org
@@ -1 +1 @@
-    (old)
+    (new)
diff --git a/mod.nix b/mod.nix
index 5555555555555555555555555555555555555555..6666666666666666666666666666666666666666 100644
--- a/mod.nix
+++ b/mod.nix
@@ -1 +1 @@
-  (old)
+  (new)
";
        let mut files = diff::parse(text).unwrap();
        let links = prepare(&mut files, Some("lit.org"));
        assert_eq!(links.get(&(0, 0, 1)), Some(&vec![(2, 0, 1)]));
        assert_eq!(links.get(&(1, 0, 1)), Some(&vec![(2, 0, 1)]));
    }

    #[test]
    fn no_mirror_no_links() {
        let mut files = diff::parse(DIFF).unwrap();
        assert!(prepare(&mut files, None).is_empty());
        assert!(prepare(&mut files, Some("missing.org")).is_empty());
        assert_eq!(files[0].path, "a.nix");
    }
}
