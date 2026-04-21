use std::path::PathBuf;

use anyhow::{bail, Context, Result};
use argh::FromArgs;
use crossterm::event::{self, Event, KeyEvent};
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};

mod app;
mod config;
mod dest;
mod file_ops;
mod inbox;
mod quicklook;
mod ui;

use app::{App, AppAction, MAX_LIST_HEIGHT};

/// A keyboard-driven TUI for sorting documents into an archive.
#[derive(FromArgs, Debug)]
struct Args {
    /// path to the configuration file
    #[argh(option, short = 'c')]
    config: Option<PathBuf>,
}

fn main() -> Result<()> {
    let args: Args = argh::from_env();

    let config_path = resolve_config_path(args.config)?;
    let cfg = config::load_config(&config_path)?;

    let files = inbox::scan_inboxes(&cfg)?;

    if files.is_empty() {
        println!("No files in inbox.");
        return Ok(());
    }

    let dest_index = dest::DestIndex::new(&cfg.destination.root)?;

    let mut app = App::new(files, dest_index, &cfg);

    run_event_loop(&mut app)
}

/// Resolves the configuration file path from the CLI option or default search.
fn resolve_config_path(explicit: Option<PathBuf>) -> Result<PathBuf> {
    if let Some(path) = explicit {
        return Ok(path);
    }
    let Some(path) = config::find_default_config() else {
        bail!("no config file found; create .docsort.toml in the current directory or home directory, or pass --config");
    };
    Ok(path)
}

/// Initializes an inline terminal suitable for the TUI viewport.
fn init_terminal() -> Result<DefaultTerminal> {
    let terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(MAX_LIST_HEIGHT),
    });
    Ok(terminal)
}

/// Runs the main TUI event loop with the commit-and-reinit pattern.
fn run_event_loop(app: &mut App) -> Result<()> {
    let mut terminal = init_terminal()?;

    loop {
        terminal
            .draw(|frame| ui::render(frame, app))
            .context("failed to draw frame")?;

        let key_event = read_key_event()?;
        let Some(key) = key_event else {
            continue;
        };

        let action = app.handle_event(key);
        match action {
            AppAction::Continue => {}
            AppAction::CommitMove { src, dest, name } => {
                // Tear down the terminal so the summary line enters the scroll buffer.
                drop(terminal);
                ratatui::restore();

                let src_name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| src.to_string_lossy().into_owned());
                let dest_display = dest
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dest.to_string_lossy().into_owned());
                println!("\u{2713} {src_name} \u{2192} {dest_display}/{name}");

                // Reinitialize the terminal.
                terminal = init_terminal()?;
            }
            AppAction::Quit => {
                drop(terminal);
                ratatui::restore();
                return Ok(());
            }
        }
    }
}

/// Reads the next key event from crossterm, blocking until one arrives.
///
/// Returns `None` for non-key events (mouse, resize, focus, paste).
fn read_key_event() -> Result<Option<KeyEvent>> {
    let event = event::read().context("failed to read terminal event")?;
    match event {
        Event::Key(key) => Ok(Some(key)),
        Event::FocusGained
        | Event::FocusLost
        | Event::Mouse(_)
        | Event::Paste(_)
        | Event::Resize(_, _) => Ok(None),
    }
}
