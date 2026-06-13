use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

use hunkle::app::{App, Outcome};
use hunkle::{backend, headless, ui};

#[derive(Parser)]
#[command(name = "hunkle", version)]
struct Args {
    #[arg(long, global = true, default_value = "git")]
    backend: String,
    #[arg(short = 'C', long = "dir", global = true, default_value = ".")]
    dir: PathBuf,
    #[command(subcommand)]
    cmd: Option<Cmd>,
}

#[derive(clap::Subcommand)]
enum Cmd {
    Dump,
    Apply,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let backend = backend::create(&args.backend, &args.dir)?;
    match args.cmd {
        Some(Cmd::Dump) => {
            println!("{}", headless::dump(backend.as_ref())?);
            return Ok(());
        }
        Some(Cmd::Apply) => {
            let input = std::io::read_to_string(std::io::stdin())?;
            println!("{}", headless::apply(backend.as_ref(), &input)?);
            return Ok(());
        }
        None => {}
    }
    let state = backend.read_state()?;
    let mut app = App::new(state);
    if app.pending().is_empty() {
        println!("hunkle: nothing to do — no staged text changes found.");
        return Ok(());
    }

    let mut terminal = ratatui::init();
    let outcome = run(&mut terminal, &mut app, backend.as_ref());
    ratatui::restore();

    match outcome? {
        Outcome::Quit => {
            println!("hunkle: aborted — no commits created, staged changes untouched.");
        }
        Outcome::Committed {
            commits,
            worktree_skipped,
        } => {
            println!("hunkle: created {} commit(s):", commits.len());
            for (id, msg, branch) in &commits {
                let dest = match branch {
                    Some(b) => format!(" (on new branch {b})"),
                    None => String::new(),
                };
                println!("  {} {}{}", &id[..10.min(id.len())], msg, dest);
            }
            let left = app.unassigned_count();
            if left > 0 {
                println!("{left} change line(s) were not assigned and remain staged.");
            }
            if !worktree_skipped.is_empty() {
                println!(
                    "warning: kept branch-assigned lines in the working tree of {} (unstaged edits present); remove them manually.",
                    worktree_skipped.join(", ")
                );
            }
        }
    }
    Ok(())
}

fn run(
    terminal: &mut ratatui::DefaultTerminal,
    app: &mut App,
    backend: &dyn backend::Backend,
) -> Result<Outcome> {
    loop {
        terminal.draw(|f| ui::draw(f, app))?;
        if let Event::Key(key) = event::read()?
            && key.kind == KeyEventKind::Press
        {
            app.handle_key(key);
        }
        if app.request_commit {
            app.request_commit = false;
            let plan = app.plan();
            if plan.commits.is_empty() {
                app.status = Some("nothing assigned — no commits to create".to_string());
            } else {
                match backend.create_commits(&plan) {
                    Ok(created) => {
                        let commits = created
                            .ids
                            .into_iter()
                            .zip(&plan.commits)
                            .map(|(id, c)| (id, c.message.clone(), c.branch.clone()))
                            .collect();
                        app.outcome = Some(Outcome::Committed {
                            commits,
                            worktree_skipped: created.worktree_skipped,
                        });
                    }
                    Err(e) => app.status = Some(format!("commit failed: {e}")),
                }
            }
        }
        if let Some(outcome) = app.outcome.take() {
            return Ok(outcome);
        }
    }
}
