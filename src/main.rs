use std::io::Write;
use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use argh::FromArgs;
use crossterm::event::{self, Event, KeyEvent};
use crossterm::style::{Attribute, Color, Print, ResetColor, SetAttribute, SetForegroundColor};
use ratatui::{DefaultTerminal, TerminalOptions, Viewport};

mod app;
mod config;
mod dest;
mod file_ops;
mod inbox;
mod quicklook;
mod ui;

use app::{viewport_height, App, AppAction};

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
        println!("Nothing to sort.");
        return Ok(());
    }

    print_startup_summary(&cfg, &files)?;

    let dest_index = dest::DestIndex::new(&cfg.destination.root)?;

    let mut app = App::new(files, dest_index, &cfg);

    run_event_loop(&mut app)
}

/// Prints the startup summary to stdout before the TUI launches.
///
/// For a single inbox, prints one line; for multiple inboxes, prints a header
/// followed by one indented line per inbox.
fn print_startup_summary(cfg: &config::Config, files: &[inbox::InboxFile]) -> Result<()> {
    let mut stdout = std::io::stdout();

    if cfg.inboxes.len() == 1 {
        let inbox = &cfg.inboxes[0];
        let label = config::inbox_label(inbox);
        let count = files.len();
        crossterm::execute!(
            stdout,
            Print("Sorting "),
            SetAttribute(Attribute::Bold),
            Print(label),
            SetAttribute(Attribute::Reset),
            Print(" \u{2014} "),
            SetAttribute(Attribute::Bold),
            SetForegroundColor(Color::Yellow),
            Print(count),
            Print(if count == 1 { " file" } else { " files" }),
            ResetColor,
            SetAttribute(Attribute::Reset),
            Print("\n"),
        )
        .context("failed to write startup summary")?;
    } else {
        crossterm::execute!(stdout, Print("Sorting:\n"))
            .context("failed to write startup summary")?;

        for inbox in &cfg.inboxes {
            let label = config::inbox_label(inbox);
            let count = files.iter().filter(|f| f.label == label).count();
            crossterm::execute!(
                stdout,
                Print("  "),
                SetAttribute(Attribute::Bold),
                Print(label),
                SetAttribute(Attribute::Reset),
                Print(" \u{2014} "),
                SetAttribute(Attribute::Bold),
                SetForegroundColor(Color::Yellow),
                Print(count),
                Print(if count == 1 { " file" } else { " files" }),
                ResetColor,
                SetAttribute(Attribute::Reset),
                Print("\n"),
            )
            .context("failed to write startup summary")?;
        }
    }

    stdout.flush().context("failed to flush stdout")?;
    Ok(())
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

/// Initializes an inline terminal with a viewport of the given height.
fn init_terminal(height: u16) -> Result<DefaultTerminal> {
    let terminal = ratatui::init_with_options(TerminalOptions {
        viewport: Viewport::Inline(height),
    });
    Ok(terminal)
}

/// Clears the inline viewport by moving the cursor up and erasing to end of screen.
///
/// Must be called before dropping the terminal so stale content from the old
/// viewport does not remain visible when the terminal is recreated at a
/// different height.
fn clear_viewport(height: u16) -> Result<()> {
    use crossterm::{
        cursor, execute,
        terminal::{Clear, ClearType},
    };
    let mut stdout = std::io::stdout();
    execute!(
        stdout,
        cursor::MoveUp(height.saturating_sub(1)),
        Clear(ClearType::FromCursorDown)
    )?;
    Ok(())
}

/// Runs the main TUI event loop with the commit-and-reinit pattern.
fn run_event_loop(app: &mut App) -> Result<()> {
    let mut current_height = viewport_height(app.files.len());
    let mut terminal = init_terminal(current_height)?;

    loop {
        terminal
            .draw(|frame| ui::render(frame, app))
            .context("failed to draw frame")?;

        app.tick();

        let has_event =
            event::poll(Duration::from_millis(100)).context("failed to poll terminal events")?;
        if !has_event {
            continue;
        }

        let key_event = read_key_event()?;
        let Some(key) = key_event else {
            continue;
        };

        let action = app.handle_event(key);
        match action {
            AppAction::Continue => {
                // Resize the terminal if the desired viewport height changed.
                let desired = app.desired_viewport_height();
                if desired != current_height {
                    clear_viewport(current_height)?;
                    drop(terminal);
                    crossterm::terminal::disable_raw_mode()
                        .context("failed to disable raw mode")?;
                    current_height = desired;
                    terminal = init_terminal(current_height)?;
                }
            }
            AppAction::CommitMove { src, dest, name } => {
                // Clear stale viewport content before tearing down the terminal so
                // the summary line is printed on a clean line in the scroll buffer.
                clear_viewport(current_height)?;
                drop(terminal);
                crossterm::terminal::disable_raw_mode().context("failed to disable raw mode")?;

                let src_name = src
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_else(|| src.to_string_lossy().into_owned());
                let dest_display = dest
                    .parent()
                    .map(|p| p.to_string_lossy().into_owned())
                    .unwrap_or_else(|| dest.to_string_lossy().into_owned());
                println!("\u{2713} {src_name} \u{2192} {dest_display}/{name}");

                // Reinitialize the terminal at the appropriate height.
                current_height = app.desired_viewport_height();
                terminal = init_terminal(current_height)?;
            }
            AppAction::Quit => {
                clear_viewport(current_height)?;
                drop(terminal);
                crossterm::terminal::disable_raw_mode().context("failed to disable raw mode")?;
                crossterm::execute!(std::io::stdout(), crossterm::cursor::Show)
                    .context("failed to show cursor")?;
                return Ok(());
            }
        }
    }
}

/// Reads the next available key event from crossterm.
///
/// Must only be called after a successful `event::poll`. Returns `None` for
/// non-key events (mouse, resize, focus, paste).
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
