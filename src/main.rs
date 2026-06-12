use std::path::PathBuf;

use anyhow::Result;
use clap::Parser;
use crossterm::event::{self, Event, KeyEventKind};

use hunkle::app::{App, Outcome};
use hunkle::{backend, ui};

#[derive(Parser)]
#[command(name = "hunkle", version)]
struct Args {
    #[arg(long, default_value = "git")]
    backend: String,
    #[arg(short = 'C', long = "dir", default_value = ".")]
    dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let backend = backend::create(&args.backend, &args.dir)?;
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
        Outcome::Committed(commits) => {
            println!("hunkle: created {} commit(s):", commits.len());
            for (id, msg) in &commits {
                println!("  {} {}", &id[..10.min(id.len())], msg);
            }
            let left = app.unassigned_count();
            if left > 0 {
                println!("{left} change line(s) were not assigned and remain staged.");
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
            if plan.is_empty() {
                app.status = Some("nothing assigned — no commits to create".to_string());
            } else {
                match backend.create_commits(&plan) {
                    Ok(ids) => {
                        let pairs = ids
                            .into_iter()
                            .zip(plan.iter().map(|c| c.message.clone()))
                            .collect();
                        app.outcome = Some(Outcome::Committed(pairs));
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
