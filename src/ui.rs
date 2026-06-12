use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Paragraph};

use crate::app::{App, Mode};
use crate::diff::LineKind;

pub fn draw(f: &mut Frame, app: &mut App) {
    let [main, footer] =
        Layout::vertical([Constraint::Min(0), Constraint::Length(1)]).areas(f.area());
    if matches!(app.mode, Mode::Review { .. }) {
        draw_review(f, app, main);
    } else if matches!(app.mode, Mode::Manage { .. }) {
        draw_manage(f, app, main);
    } else {
        let [diff, side] =
            Layout::horizontal([Constraint::Min(0), Constraint::Length(36)]).areas(main);
        draw_hunk(f, app, diff);
        draw_sidebar(f, app, side);
    }
    draw_footer(f, app, footer);
}

fn draw_hunk(f: &mut Frame, app: &mut App, area: Rect) {
    let pending = app.pending();
    let Some(&(fi, hi)) = pending.get(app.pos.min(pending.len().saturating_sub(1))) else {
        return;
    };
    let file = &app.files[fi];
    let hunk = &file.hunks[hi];

    let select = match &app.mode {
        Mode::Select { cursor, selected } => {
            let toggleable = app.unassigned_lines(fi, hi);
            let cursor_li = toggleable.get(*cursor).copied();
            Some((cursor_li, selected.clone()))
        }
        _ => None,
    };

    let mut lines: Vec<Line> = vec![Line::styled(
        hunk.header.clone(),
        Style::new().fg(Color::Cyan),
    )];
    let mut cursor_row = 0u16;
    for (li, l) in hunk.lines.iter().enumerate() {
        let assigned = app.assign[fi][hi][li];
        let (prefix, mut style) = match l.kind {
            LineKind::Add => ('+', Style::new().fg(Color::Green)),
            LineKind::Del => ('-', Style::new().fg(Color::Red)),
            LineKind::Context => (' ', Style::new().fg(Color::DarkGray)),
        };
        let mut cur = ' ';
        let mut sel = ' ';
        if let Some((cursor_li, selected)) = &select {
            if *cursor_li == Some(li) {
                cur = '>';
                style = style.add_modifier(Modifier::REVERSED);
                cursor_row = lines.len() as u16;
            }
            if selected.contains(&li) {
                sel = '*';
                style = style.add_modifier(Modifier::BOLD);
            }
        }
        let tag = match assigned {
            Some(ci) => {
                style = style.add_modifier(Modifier::DIM);
                format!("[{}]", App::key_label(ci))
            }
            None => String::new(),
        };
        lines.push(Line::from(vec![
            Span::styled(
                format!("{cur}{sel}{tag:<5} "),
                Style::new().fg(Color::Yellow),
            ),
            Span::styled(format!("{prefix}{}", l.content), style),
        ]));
    }

    let view_h = area.height.saturating_sub(2);
    if select.is_some() && view_h > 0 {
        if cursor_row < app.scroll {
            app.scroll = cursor_row;
        } else if cursor_row >= app.scroll + view_h {
            app.scroll = cursor_row - view_h + 1;
        }
    }
    let max_scroll = (lines.len() as u16).saturating_sub(view_h.max(1));
    app.scroll = app.scroll.min(max_scroll);

    let mode_tag = if select.is_some() {
        " [picking lines]"
    } else {
        ""
    };
    let title = format!(
        " {} — hunk {}/{}{} ",
        file.path,
        app.pos.min(pending.len() - 1) + 1,
        pending.len(),
        mode_tag,
    );
    let block = Block::bordered().title(title);
    f.render_widget(
        Paragraph::new(lines).block(block).scroll((app.scroll, 0)),
        area,
    );
}

fn draw_sidebar(f: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();
    if app.commits.is_empty() {
        lines.push(Line::styled(
            "no commits yet",
            Style::new().fg(Color::DarkGray),
        ));
        lines.push(Line::styled(
            "press n to create one",
            Style::new().fg(Color::DarkGray),
        ));
    }
    for (ci, msg) in app.commits.iter().enumerate() {
        let (add, del) = app.commit_stats(ci);
        lines.push(Line::from(vec![
            Span::styled(
                format!("[{}] ", App::key_label(ci)),
                Style::new().fg(Color::Cyan).add_modifier(Modifier::BOLD),
            ),
            Span::raw(msg.clone()),
        ]));
        lines.push(Line::styled(
            format!("      +{add} -{del}"),
            Style::new().fg(Color::DarkGray),
        ));
    }
    lines.push(Line::raw(""));
    lines.push(Line::styled(
        format!("{} line(s) unassigned", app.unassigned_count()),
        Style::new().fg(Color::DarkGray),
    ));
    let block = Block::bordered().title(format!(" commits — {} ", app.branch));
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_review(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Review { sel, edit } = &app.mode else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        "the following commits will be created (in this order):",
        Style::new().fg(Color::DarkGray),
    ));
    lines.push(Line::raw(""));
    for (ci, msg) in app.commits.iter().enumerate() {
        let (add, del) = app.commit_stats(ci);
        let empty = add + del == 0;
        let mut style = Style::new();
        if ci == *sel {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let text = match edit {
            Some(buf) if ci == *sel => format!("[{}] {buf}_", App::key_label(ci)),
            _ => format!("[{}] {msg}", App::key_label(ci)),
        };
        let note = if empty {
            "  (empty — skipped)".to_string()
        } else {
            format!("  (+{add} -{del})")
        };
        lines.push(Line::from(vec![
            Span::styled(text, style),
            Span::styled(note, Style::new().fg(Color::DarkGray)),
        ]));
    }
    lines.push(Line::raw(""));
    let left = app.unassigned_count();
    if left > 0 {
        lines.push(Line::styled(
            format!("{left} change line(s) unassigned — they will remain staged."),
            Style::new().fg(Color::Yellow),
        ));
    }
    lines.push(Line::styled(
        format!("committing onto: {}", app.branch),
        Style::new().fg(Color::DarkGray),
    ));
    let block = Block::bordered().title(" review ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_manage(f: &mut Frame, app: &App, area: Rect) {
    let Mode::Manage { cursor, mark, .. } = &app.mode else {
        return;
    };
    let mut lines: Vec<Line> = Vec::new();
    lines.push(Line::styled(
        "swap two commits' keys (this also swaps their creation order):",
        Style::new().fg(Color::DarkGray),
    ));
    lines.push(Line::raw(""));
    for (ci, msg) in app.commits.iter().enumerate() {
        let (add, del) = app.commit_stats(ci);
        let marked = *mark == Some(ci);
        let mut style = Style::new();
        if marked {
            style = style.fg(Color::Yellow).add_modifier(Modifier::BOLD);
        }
        if ci == *cursor {
            style = style.add_modifier(Modifier::REVERSED);
        }
        let marker = if marked { '*' } else { ' ' };
        lines.push(Line::from(vec![
            Span::styled(format!("{marker}[{}] {msg}", App::key_label(ci)), style),
            Span::styled(
                format!("  (+{add} -{del})"),
                Style::new().fg(Color::DarkGray),
            ),
        ]));
    }
    let block = Block::bordered().title(" manage commits ");
    f.render_widget(Paragraph::new(lines).block(block), area);
}

fn draw_footer(f: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(digits) = &app.ext_input {
        Line::from(vec![
            Span::styled("assign to commit: 0", Style::new().fg(Color::Cyan)),
            Span::raw(digits.clone()),
            Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            Span::styled(
                "   (digits, enter: confirm, esc: cancel)",
                Style::new().fg(Color::DarkGray),
            ),
        ])
    } else if let Mode::Name { input, .. } = &app.mode {
        Line::from(vec![
            Span::styled("new commit message: ", Style::new().fg(Color::Cyan)),
            Span::raw(input.clone()),
            Span::styled("_", Style::new().add_modifier(Modifier::SLOW_BLINK)),
            Span::styled(
                "   (enter: confirm, esc: cancel)",
                Style::new().fg(Color::DarkGray),
            ),
        ])
    } else if let Some(status) = &app.status {
        Line::styled(status.clone(), Style::new().fg(Color::Yellow))
    } else {
        let hints = match &app.mode {
            Mode::Browse => {
                "1-9/0<id> assign hunk | n new commit | v pick lines | s/h skip/back | j/k scroll | u unassign | m manage | d review | q quit"
            }
            Mode::Select { .. } => {
                "space toggle | a all | 1-9/0<id> assign selection | n new commit | j/k move | esc cancel"
            }
            Mode::Review { edit: Some(_), .. } => "enter: save message, esc: cancel",
            Mode::Review { .. } => {
                "enter create commits | e edit message | m manage | j/k move | esc back | q quit"
            }
            Mode::Manage { mark: Some(_), .. } => {
                "enter/space swap with marked | j/k move | esc back"
            }
            Mode::Manage { .. } => "enter/space mark commit | j/k move | esc back",
            Mode::Name { .. } => unreachable!(),
        };
        Line::styled(hints, Style::new().fg(Color::DarkGray))
    };
    f.render_widget(Paragraph::new(line), area);
}
