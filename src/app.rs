use std::collections::{HashMap, HashSet};

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

use crate::backend::RepoState;
use crate::diff::{FileDiff, LineKind};
use crate::model::{Assignments, CommitSpec, Plan, build_plan, empty_assignments};

#[derive(Debug)]
pub enum Mode {
    Browse,
    Select {
        cursor: usize,
        selected: HashSet<usize>,
    },
    Branch {
        input: String,
        lines: Option<Vec<usize>>,
    },
    Name {
        input: String,
        lines: Option<Vec<usize>>,
        branch: Option<String>,
    },
    Review {
        sel: usize,
        edit: Option<String>,
    },
    Manage {
        cursor: usize,
        mark: Option<usize>,
        back: Option<usize>,
    },
    Assigned {
        cursor: usize,
    },
}

type Snapshot = (Vec<CommitSpec>, Assignments);

pub enum Outcome {
    Quit,
    Committed {
        commits: Vec<(String, String, Option<String>)>,
        worktree_skipped: Vec<String>,
    },
}

pub struct App {
    pub files: Vec<FileDiff>,
    pub bases: HashMap<String, Vec<u8>>,
    pub branch: String,
    pub assign: Assignments,
    pub commits: Vec<CommitSpec>,
    pub pos: usize,
    pub scroll: u16,
    pub mode: Mode,
    pub status: Option<String>,
    pub request_commit: bool,
    pub outcome: Option<Outcome>,
    pub ext_input: Option<String>,
    undo_stack: Vec<Snapshot>,
}

impl App {
    pub fn new(state: RepoState) -> Self {
        let binary = state.files.iter().filter(|f| f.binary).count();
        let assign = empty_assignments(&state.files);
        App {
            files: state.files,
            bases: state.bases,
            branch: state.branch,
            assign,
            commits: Vec::new(),
            pos: 0,
            scroll: 0,
            mode: Mode::Browse,
            status: (binary > 0)
                .then(|| format!("skipped {binary} binary file(s); they stay staged")),
            request_commit: false,
            outcome: None,
            ext_input: None,
            undo_stack: Vec::new(),
        }
    }

    pub fn key_label(ci: usize) -> String {
        if ci < 9 {
            (ci + 1).to_string()
        } else {
            format!("0{}", ci + 1)
        }
    }

    pub fn pending(&self) -> Vec<(usize, usize)> {
        let mut out = Vec::new();
        for (fi, f) in self.files.iter().enumerate() {
            for (hi, h) in f.hunks.iter().enumerate() {
                let open = h
                    .lines
                    .iter()
                    .enumerate()
                    .any(|(li, l)| l.is_change() && self.assign[fi][hi][li].is_none());
                if open {
                    out.push((fi, hi));
                }
            }
        }
        out
    }

    pub fn current(&self) -> Option<(usize, usize)> {
        let pending = self.pending();
        pending
            .get(self.pos.min(pending.len().saturating_sub(1)))
            .copied()
    }

    pub fn unassigned_lines(&self, fi: usize, hi: usize) -> Vec<usize> {
        self.files[fi].hunks[hi]
            .lines
            .iter()
            .enumerate()
            .filter(|(li, l)| l.is_change() && self.assign[fi][hi][*li].is_none())
            .map(|(li, _)| li)
            .collect()
    }

    pub fn has_assignments(&self) -> bool {
        self.assign.iter().flatten().flatten().any(Option::is_some)
    }

    pub fn commit_stats(&self, ci: usize) -> (usize, usize) {
        let (mut add, mut del) = (0, 0);
        for (fi, f) in self.files.iter().enumerate() {
            for (hi, h) in f.hunks.iter().enumerate() {
                for (li, l) in h.lines.iter().enumerate() {
                    if self.assign[fi][hi][li] == Some(ci) {
                        match l.kind {
                            LineKind::Add => add += 1,
                            LineKind::Del => del += 1,
                            LineKind::Context => {}
                        }
                    }
                }
            }
        }
        (add, del)
    }

    pub fn unassigned_count(&self) -> usize {
        let mut n = 0;
        for (fi, f) in self.files.iter().enumerate() {
            for (hi, h) in f.hunks.iter().enumerate() {
                for (li, l) in h.lines.iter().enumerate() {
                    if l.is_change() && self.assign[fi][hi][li].is_none() {
                        n += 1;
                    }
                }
            }
        }
        n
    }

    pub fn plan(&self) -> Plan {
        build_plan(&self.files, &self.assign, &self.bases, &self.commits)
    }

    pub fn commit_label(&self, ci: usize) -> String {
        let spec = &self.commits[ci];
        match &spec.branch {
            Some(b) => format!("{} -> {b}", spec.message),
            None => spec.message.clone(),
        }
    }

    pub fn assigned_rows(&self) -> Vec<(usize, usize, usize)> {
        let mut out = Vec::new();
        for (fi, f) in self.files.iter().enumerate() {
            for hi in 0..f.hunks.len() {
                let mut seen: Vec<usize> = Vec::new();
                for a in self.assign[fi][hi].iter().flatten() {
                    if !seen.contains(a) {
                        seen.push(*a);
                    }
                }
                seen.sort_unstable();
                for ci in seen {
                    out.push((fi, hi, ci));
                }
            }
        }
        out
    }

    fn push_undo(&mut self) {
        const LIMIT: usize = 100;
        if self.undo_stack.len() == LIMIT {
            self.undo_stack.remove(0);
        }
        self.undo_stack
            .push((self.commits.clone(), self.assign.clone()));
    }

    fn undo(&mut self) {
        let Some((commits, assign)) = self.undo_stack.pop() else {
            self.status = Some("nothing to undo".to_string());
            return;
        };
        self.commits = commits;
        self.assign = assign;
        self.ext_input = None;
        self.scroll = 0;
        let n = self.pending().len();
        self.pos = self.pos.min(n.saturating_sub(1));
        self.mode = Mode::Browse;
        self.status = Some("undid last change".to_string());
    }

    fn reassign_row(&mut self, fi: usize, hi: usize, from: usize, to: usize) {
        let mut count = 0;
        for a in self.assign[fi][hi].iter_mut() {
            if *a == Some(from) {
                *a = Some(to);
                count += 1;
            }
        }
        self.status = Some(format!(
            "moved {count} line(s) to [{}] {}",
            Self::key_label(to),
            self.commit_label(to)
        ));
    }

    fn unassign_row(&mut self, fi: usize, hi: usize, ci: usize) {
        let mut count = 0;
        for a in self.assign[fi][hi].iter_mut() {
            if *a == Some(ci) {
                *a = None;
                count += 1;
            }
        }
        self.status = Some(format!("unassigned {count} line(s)"));
    }

    fn assign_current(&mut self, commit: usize, lines: Option<Vec<usize>>) {
        let Some((fi, hi)) = self.current() else {
            return;
        };
        let targets = lines.unwrap_or_else(|| self.unassigned_lines(fi, hi));
        let count = targets.len();
        for li in targets {
            self.assign[fi][hi][li] = Some(commit);
        }
        self.status = Some(format!(
            "{count} line(s) -> [{}] {}",
            Self::key_label(commit),
            self.commit_label(commit)
        ));
        self.after_assign();
    }

    fn assign_via_key(&mut self, ci: usize) {
        if ci >= self.commits.len() {
            self.status = Some(format!(
                "no commit [{}] yet — press n to create one",
                Self::key_label(ci)
            ));
            return;
        }
        match &self.mode {
            Mode::Select { selected, .. } => {
                if selected.is_empty() {
                    self.status = Some("no lines selected (space toggles)".to_string());
                } else {
                    let mut lines: Vec<usize> = selected.iter().copied().collect();
                    lines.sort_unstable();
                    self.push_undo();
                    self.assign_current(ci, Some(lines));
                }
            }
            Mode::Assigned { cursor } => {
                let cursor = *cursor;
                if let Some(&(fi, hi, from)) = self.assigned_rows().get(cursor) {
                    if from == ci {
                        self.status = Some("already on that commit".to_string());
                    } else {
                        self.push_undo();
                        self.reassign_row(fi, hi, from, ci);
                    }
                }
            }
            _ => {
                self.push_undo();
                self.assign_current(ci, None);
            }
        }
    }

    fn start_ext(&mut self) {
        if self.commits.is_empty() {
            self.status = Some("no commits yet — press n to create one".to_string());
        } else {
            self.ext_input = Some(String::new());
        }
    }

    fn key_ext(&mut self, key: KeyEvent) {
        let max_id = self.commits.len();
        let Some(digits) = &mut self.ext_input else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.ext_input = None,
            KeyCode::Backspace => {
                if digits.pop().is_none() {
                    self.ext_input = None;
                }
            }
            KeyCode::Char(c @ '0'..='9') => {
                digits.push(c);
                let id = digits.parse::<usize>().unwrap_or(usize::MAX);
                if id > max_id {
                    self.status = Some(format!(
                        "no commit [0{digits}] (highest is [{}])",
                        Self::key_label(self.commits.len() - 1)
                    ));
                    self.ext_input = None;
                } else if id >= 1 && id.saturating_mul(10) > max_id {
                    self.ext_input = None;
                    self.assign_via_key(id - 1);
                }
            }
            KeyCode::Enter => {
                let id = digits.parse::<usize>().unwrap_or(0);
                self.ext_input = None;
                if (1..=max_id).contains(&id) {
                    self.assign_via_key(id - 1);
                } else {
                    self.status = Some("invalid commit id".to_string());
                }
            }
            _ => {}
        }
    }

    fn after_assign(&mut self) {
        self.scroll = 0;
        let n = self.pending().len();
        if n == 0 {
            self.mode = Mode::Review { sel: 0, edit: None };
        } else {
            self.pos = self.pos.min(n - 1);
            self.mode = Mode::Browse;
        }
    }

    pub fn handle_key(&mut self, key: KeyEvent) {
        self.status = None;
        if key.code == KeyCode::Char('c') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.outcome = Some(Outcome::Quit);
            return;
        }
        if key.code == KeyCode::Char('z') && key.modifiers.contains(KeyModifiers::CONTROL) {
            self.undo();
            return;
        }
        if self.ext_input.is_some() {
            self.key_ext(key);
            return;
        }
        match &mut self.mode {
            Mode::Browse => self.key_browse(key),
            Mode::Select { .. } => self.key_select(key),
            Mode::Branch { .. } => self.key_branch(key),
            Mode::Name { .. } => self.key_name(key),
            Mode::Review { .. } => self.key_review(key),
            Mode::Manage { .. } => self.key_manage(key),
            Mode::Assigned { .. } => self.key_assigned(key),
        }
    }

    fn swap_commits(&mut self, a: usize, b: usize) {
        self.commits.swap(a, b);
        for file in self.assign.iter_mut() {
            for hunk in file.iter_mut() {
                for line in hunk.iter_mut() {
                    if *line == Some(a) {
                        *line = Some(b);
                    } else if *line == Some(b) {
                        *line = Some(a);
                    }
                }
            }
        }
    }

    fn key_manage(&mut self, key: KeyEvent) {
        let n = self.commits.len();
        let Mode::Manage { cursor, mark, back } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('m') => {
                self.mode = match *back {
                    Some(sel) => Mode::Review { sel, edit: None },
                    None => Mode::Browse,
                };
            }
            KeyCode::Char('j') | KeyCode::Down => *cursor = (*cursor + 1).min(n.saturating_sub(1)),
            KeyCode::Char('k') | KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Enter | KeyCode::Char(' ') => match *mark {
                None => *mark = Some(*cursor),
                Some(m) if m == *cursor => *mark = None,
                Some(m) => {
                    let cur = *cursor;
                    *mark = None;
                    self.push_undo();
                    self.swap_commits(m, cur);
                    self.status = Some(format!(
                        "swapped [{}] <-> [{}]",
                        Self::key_label(m),
                        Self::key_label(cur)
                    ));
                }
            },
            _ => {}
        }
    }

    fn open_assigned(&mut self) {
        if self.has_assignments() {
            self.mode = Mode::Assigned { cursor: 0 };
        } else {
            self.status = Some("nothing assigned yet".to_string());
        }
    }

    fn key_assigned(&mut self, key: KeyEvent) {
        let rows = self.assigned_rows();
        let Mode::Assigned { cursor } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') | KeyCode::Char('a') => self.mode = Mode::Browse,
            KeyCode::Char('j') | KeyCode::Down => {
                *cursor = (*cursor + 1).min(rows.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Char(c @ '1'..='9') => self.assign_via_key(c as usize - '1' as usize),
            KeyCode::Char('0') => self.start_ext(),
            KeyCode::Char('e') => {
                if let Some(&(_, _, ci)) = rows.get(*cursor) {
                    self.mode = Mode::Review {
                        sel: ci,
                        edit: Some(self.commits[ci].message.clone()),
                    };
                }
            }
            KeyCode::Char('u') => {
                if let Some(&(fi, hi, ci)) = rows.get(*cursor) {
                    self.push_undo();
                    self.unassign_row(fi, hi, ci);
                    let n = self.assigned_rows().len();
                    if n == 0 {
                        self.mode = Mode::Browse;
                    } else if let Mode::Assigned { cursor } = &mut self.mode {
                        *cursor = (*cursor).min(n - 1);
                    }
                }
            }
            _ => {}
        }
    }

    fn key_browse(&mut self, key: KeyEvent) {
        let pending_len = self.pending().len();
        match key.code {
            KeyCode::Char('q') | KeyCode::Esc => self.outcome = Some(Outcome::Quit),
            KeyCode::Char(c @ '1'..='9') => self.assign_via_key(c as usize - '1' as usize),
            KeyCode::Char('0') => self.start_ext(),
            KeyCode::Char('n') => {
                self.mode = Mode::Name {
                    input: String::new(),
                    lines: None,
                    branch: None,
                };
            }
            KeyCode::Char('b') => {
                self.mode = Mode::Branch {
                    input: String::new(),
                    lines: None,
                };
            }
            KeyCode::Char('v') | KeyCode::Char(' ') => {
                self.mode = Mode::Select {
                    cursor: 0,
                    selected: HashSet::new(),
                };
            }
            KeyCode::Char('s') | KeyCode::Char('l') | KeyCode::Right | KeyCode::Tab => {
                if pending_len > 0 {
                    self.pos = (self.pos + 1) % pending_len;
                    self.scroll = 0;
                }
            }
            KeyCode::Char('h') | KeyCode::Left => {
                if pending_len > 0 {
                    self.pos = (self.pos + pending_len - 1) % pending_len;
                    self.scroll = 0;
                }
            }
            KeyCode::Char('j') | KeyCode::Down => self.scroll = self.scroll.saturating_add(1),
            KeyCode::Char('k') | KeyCode::Up => self.scroll = self.scroll.saturating_sub(1),
            KeyCode::Char('u') => {
                if let Some((fi, hi)) = self.current() {
                    let has = self.assign[fi][hi].iter().any(Option::is_some);
                    if has {
                        self.push_undo();
                    }
                    let mut n = 0;
                    for a in self.assign[fi][hi].iter_mut() {
                        if a.is_some() {
                            *a = None;
                            n += 1;
                        }
                    }
                    self.status = Some(if n > 0 {
                        format!("unassigned {n} line(s) of this hunk")
                    } else {
                        "nothing assigned in this hunk".to_string()
                    });
                }
            }
            KeyCode::Char('a') => self.open_assigned(),
            KeyCode::Char('m') => {
                if self.commits.is_empty() {
                    self.status = Some("no commits to manage yet".to_string());
                } else {
                    self.mode = Mode::Manage {
                        cursor: 0,
                        mark: None,
                        back: None,
                    };
                }
            }
            KeyCode::Char('d') | KeyCode::Enter => {
                if self.has_assignments() {
                    self.mode = Mode::Review { sel: 0, edit: None };
                } else {
                    self.status = Some("nothing assigned yet".to_string());
                }
            }
            _ => {}
        }
    }

    fn key_select(&mut self, key: KeyEvent) {
        let Some((fi, hi)) = self.current() else {
            self.mode = Mode::Browse;
            return;
        };
        let toggleable = self.unassigned_lines(fi, hi);
        let Mode::Select { cursor, selected } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc | KeyCode::Char('v') | KeyCode::Char('q') => self.mode = Mode::Browse,
            KeyCode::Char('j') | KeyCode::Down => {
                *cursor = (*cursor + 1).min(toggleable.len().saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => *cursor = cursor.saturating_sub(1),
            KeyCode::Char(' ') => {
                if let Some(&li) = toggleable.get(*cursor) {
                    if !selected.remove(&li) {
                        selected.insert(li);
                    }
                    *cursor = (*cursor + 1).min(toggleable.len().saturating_sub(1));
                }
            }
            KeyCode::Char('a') => {
                if selected.len() == toggleable.len() {
                    selected.clear();
                } else {
                    selected.extend(toggleable.iter().copied());
                }
            }
            KeyCode::Char(c @ '1'..='9') => self.assign_via_key(c as usize - '1' as usize),
            KeyCode::Char('0') => self.start_ext(),
            KeyCode::Char('n') | KeyCode::Char('b') => {
                if selected.is_empty() {
                    self.status = Some("no lines selected (space toggles)".to_string());
                } else {
                    let mut lines: Vec<usize> = selected.iter().copied().collect();
                    lines.sort_unstable();
                    self.mode = if key.code == KeyCode::Char('b') {
                        Mode::Branch {
                            input: String::new(),
                            lines: Some(lines),
                        }
                    } else {
                        Mode::Name {
                            input: String::new(),
                            lines: Some(lines),
                            branch: None,
                        }
                    };
                }
            }
            _ => {}
        }
    }

    fn back_from_input(lines: Option<Vec<usize>>) -> Mode {
        match lines {
            Some(lines) => Mode::Select {
                cursor: 0,
                selected: lines.into_iter().collect(),
            },
            None => Mode::Browse,
        }
    }

    fn key_branch(&mut self, key: KeyEvent) {
        let Mode::Branch { input, lines } = &mut self.mode else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Self::back_from_input(lines.take()),
            KeyCode::Enter => {
                let name = input.trim().to_string();
                if name.is_empty() {
                    self.status = Some("branch name cannot be empty".to_string());
                    return;
                }
                if name.contains(char::is_whitespace) {
                    self.status = Some("branch name cannot contain whitespace".to_string());
                    return;
                }
                self.mode = Mode::Name {
                    input: String::new(),
                    lines: lines.take(),
                    branch: Some(name),
                };
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            _ => {}
        }
    }

    fn key_name(&mut self, key: KeyEvent) {
        let Mode::Name {
            input,
            lines,
            branch,
        } = &mut self.mode
        else {
            return;
        };
        match key.code {
            KeyCode::Esc => self.mode = Self::back_from_input(lines.take()),
            KeyCode::Enter => {
                let msg = input.trim().to_string();
                if msg.is_empty() {
                    self.status = Some("commit message cannot be empty".to_string());
                    return;
                }
                let lines = lines.take();
                let branch = branch.take();
                self.push_undo();
                self.commits.push(CommitSpec {
                    message: msg,
                    branch,
                });
                self.assign_current(self.commits.len() - 1, lines);
            }
            KeyCode::Backspace => {
                input.pop();
            }
            KeyCode::Char(c) => input.push(c),
            _ => {}
        }
    }

    fn key_review(&mut self, key: KeyEvent) {
        let n_commits = self.commits.len();
        let pending_left = !self.pending().is_empty();
        let Mode::Review { sel, edit } = &mut self.mode else {
            return;
        };
        if let Some(buf) = edit {
            match key.code {
                KeyCode::Esc => *edit = None,
                KeyCode::Enter => {
                    let msg = buf.trim().to_string();
                    if msg.is_empty() {
                        self.status = Some("commit message cannot be empty".to_string());
                        return;
                    }
                    self.commits[*sel].message = msg;
                    *edit = None;
                }
                KeyCode::Backspace => {
                    buf.pop();
                }
                KeyCode::Char(c) => buf.push(c),
                _ => {}
            }
            return;
        }
        match key.code {
            KeyCode::Char('q') => self.outcome = Some(Outcome::Quit),
            KeyCode::Char('j') | KeyCode::Down => {
                *sel = (*sel + 1).min(n_commits.saturating_sub(1));
            }
            KeyCode::Char('k') | KeyCode::Up => *sel = sel.saturating_sub(1),
            KeyCode::Char('e') => {
                if n_commits > 0 {
                    *edit = Some(self.commits[*sel].message.clone());
                }
            }
            KeyCode::Char('m') => {
                if n_commits > 0 {
                    self.mode = Mode::Manage {
                        cursor: 0,
                        mark: None,
                        back: Some(*sel),
                    };
                }
            }
            KeyCode::Char('a') => self.open_assigned(),
            KeyCode::Esc | KeyCode::Char('b') => {
                if pending_left {
                    self.mode = Mode::Browse;
                } else {
                    self.status = Some(
                        "all hunks assigned (u on review is not supported; q to quit)".to_string(),
                    );
                }
            }
            KeyCode::Enter | KeyCode::Char('y') => self.request_commit = true,
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::diff;

    fn app_with_hunks(n: usize) -> App {
        let mut text = String::from(
            "diff --git a/f.txt b/f.txt\nindex 1111..2222 100644\n--- a/f.txt\n+++ b/f.txt\n",
        );
        for i in 0..n {
            text.push_str(&format!(
                "@@ -{0},1 +{0},1 @@\n-a{1}\n+b{1}\n",
                i * 10 + 1,
                i
            ));
        }
        let files = diff::parse(&text).unwrap();
        App::new(RepoState {
            files,
            bases: HashMap::new(),
            branch: "main".to_string(),
        })
    }

    fn press(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::from(code));
    }

    fn press_ctrl(app: &mut App, code: KeyCode) {
        app.handle_key(KeyEvent::new(code, KeyModifiers::CONTROL));
    }

    fn name_commit(app: &mut App, name: &str) {
        press(app, KeyCode::Char('n'));
        for c in name.chars() {
            press(app, KeyCode::Char(c));
        }
        press(app, KeyCode::Enter);
    }

    #[test]
    fn key_labels() {
        assert_eq!(App::key_label(0), "1");
        assert_eq!(App::key_label(8), "9");
        assert_eq!(App::key_label(9), "010");
        assert_eq!(App::key_label(10), "011");
        assert_eq!(App::key_label(18), "019");
    }

    #[test]
    fn extended_ids_address_commits_by_number() {
        let mut app = app_with_hunks(25);
        for i in 0..19 {
            name_commit(&mut app, &format!("c{i}"));
        }
        assert_eq!(app.commits.len(), 19);

        let (fi, hi) = app.current().unwrap();
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Char('1'));
        assert!(app.ext_input.is_some());
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.assign[fi][hi][0], Some(0));

        let (fi, hi) = app.current().unwrap();
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Char('0'));
        assert!(app.ext_input.is_none());
        assert_eq!(app.assign[fi][hi][0], Some(9));

        let (fi, hi) = app.current().unwrap();
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Char('2'));
        assert!(app.ext_input.is_none());
        assert_eq!(app.assign[fi][hi][0], Some(1));

        let (fi, hi) = app.current().unwrap();
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Char('1'));
        press(&mut app, KeyCode::Char('9'));
        assert!(app.ext_input.is_none());
        assert_eq!(app.assign[fi][hi][0], Some(18));

        let (fi, hi) = app.current().unwrap();
        press(&mut app, KeyCode::Char('0'));
        press(&mut app, KeyCode::Enter);
        assert_eq!(app.assign[fi][hi][0], None);
        assert!(app.status.is_some());
    }

    #[test]
    fn zero_without_commits_is_an_error() {
        let mut app = app_with_hunks(3);
        press(&mut app, KeyCode::Char('0'));
        assert!(app.ext_input.is_none());
        assert!(app.status.is_some());
    }

    #[test]
    fn manage_swaps_commit_positions_and_assignments() {
        let mut app = app_with_hunks(4);
        name_commit(&mut app, "c0");
        name_commit(&mut app, "c1");
        name_commit(&mut app, "c2");
        assert_eq!(app.assign[0][1][0], Some(1));
        assert_eq!(app.assign[0][2][0], Some(2));

        press(&mut app, KeyCode::Char('m'));
        assert!(matches!(app.mode, Mode::Manage { .. }));
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);
        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Enter);

        let messages: Vec<&str> = app.commits.iter().map(|c| c.message.as_str()).collect();
        assert_eq!(messages, vec!["c0", "c2", "c1"]);
        assert_eq!(app.assign[0][1][0], Some(2));
        assert_eq!(app.assign[0][2][0], Some(1));

        press(&mut app, KeyCode::Esc);
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn branch_key_creates_commit_on_new_branch() {
        let mut app = app_with_hunks(2);
        press(&mut app, KeyCode::Char('b'));
        for c in "topic".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Name { .. }));
        for c in "fix".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);

        assert_eq!(app.commits.len(), 1);
        assert_eq!(app.commits[0].message, "fix");
        assert_eq!(app.commits[0].branch.as_deref(), Some("topic"));
        assert_eq!(app.assign[0][0][0], Some(0));
    }

    #[test]
    fn undo_reverts_last_assignment() {
        let mut app = app_with_hunks(3);
        name_commit(&mut app, "c0");
        let snapshot = app.assign.clone();
        let (fi, hi) = app.current().unwrap();
        press(&mut app, KeyCode::Char('1'));
        assert_eq!(app.assign[fi][hi][0], Some(0));

        press_ctrl(&mut app, KeyCode::Char('z'));
        assert_eq!(app.assign, snapshot);
        assert!(matches!(app.mode, Mode::Browse));
    }

    #[test]
    fn undo_reverts_commit_creation() {
        let mut app = app_with_hunks(2);
        name_commit(&mut app, "c0");
        assert_eq!(app.commits.len(), 1);
        assert!(app.has_assignments());

        press_ctrl(&mut app, KeyCode::Char('z'));
        assert_eq!(app.commits.len(), 0);
        assert!(!app.has_assignments());

        press_ctrl(&mut app, KeyCode::Char('z'));
        assert!(app.status.as_deref() == Some("nothing to undo"));
    }

    #[test]
    fn assigned_overview_reassign_unassign_and_edit() {
        let mut app = app_with_hunks(3);
        name_commit(&mut app, "c0");
        name_commit(&mut app, "c1");
        assert_eq!(app.assigned_rows(), vec![(0, 0, 0), (0, 1, 1)]);

        press(&mut app, KeyCode::Char('a'));
        assert!(matches!(app.mode, Mode::Assigned { cursor: 0 }));

        press(&mut app, KeyCode::Char('2'));
        assert_eq!(app.assign[0][0][0], Some(1));
        assert_eq!(app.assigned_rows(), vec![(0, 0, 1), (0, 1, 1)]);

        press(&mut app, KeyCode::Char('j'));
        press(&mut app, KeyCode::Char('u'));
        assert_eq!(app.assign[0][1][0], None);
        assert_eq!(app.assigned_rows(), vec![(0, 0, 1)]);
        assert!(matches!(app.mode, Mode::Assigned { cursor: 0 }));

        press(&mut app, KeyCode::Char('e'));
        assert!(matches!(
            app.mode,
            Mode::Review {
                sel: 1,
                edit: Some(_)
            }
        ));

        press_ctrl(&mut app, KeyCode::Char('z'));
        assert_eq!(app.assign[0][1][0], Some(1));
    }

    #[test]
    fn assigned_overview_empties_back_to_browse() {
        let mut app = app_with_hunks(1);
        name_commit(&mut app, "c0");
        press(&mut app, KeyCode::Char('a'));
        assert!(matches!(app.mode, Mode::Assigned { .. }));
        press(&mut app, KeyCode::Char('u'));
        assert!(matches!(app.mode, Mode::Browse));
        assert!(!app.has_assignments());
    }

    #[test]
    fn branch_name_rejects_empty_and_whitespace() {
        let mut app = app_with_hunks(1);
        press(&mut app, KeyCode::Char('b'));
        press(&mut app, KeyCode::Enter);
        assert!(app.status.is_some());
        assert!(matches!(app.mode, Mode::Branch { .. }));
        for c in "a b".chars() {
            press(&mut app, KeyCode::Char(c));
        }
        press(&mut app, KeyCode::Enter);
        assert!(matches!(app.mode, Mode::Branch { .. }));
    }
}
