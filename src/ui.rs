use std::time::SystemTime;

use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;
use ratatui::Frame;

use crate::app::{App, AppState};
use crate::dest::DestIndex;

/// Renders the entire TUI frame based on the current application state.
pub fn render(frame: &mut Frame, app: &App) {
    let area = frame.area();

    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(1), Constraint::Length(1)])
        .split(area);

    let content_area = chunks[0];
    let hint_area = chunks[1];

    match app.state {
        AppState::Browsing => render_browsing(frame, app, content_area),
        AppState::Searching => render_searching(frame, app, content_area),
        AppState::SubfolderCreation => render_subfolder_creation(frame, app, content_area),
        AppState::Naming => render_naming(frame, app, content_area),
    }

    render_hint_bar(frame, app, hint_area);
}

/// Renders the file browser list in Browsing mode.
fn render_browsing(frame: &mut Frame, app: &App, area: Rect) {
    let visible_rows = area.height as usize;
    let (start, end) = visible_window(app.cursor, app.files.len(), visible_rows);

    let mut lines: Vec<Line> = Vec::new();
    for i in start..end {
        let file = &app.files[i];
        let is_highlighted = i == app.cursor;
        let is_moved = app.moved.contains_key(&file.path);

        let prefix = if is_highlighted { "\u{25b6}" } else { " " };
        let label = format!("[{}]", file.label);
        let date = format_system_time(&file.modified);
        let filename = &file.filename;

        let mut spans: Vec<Span> = Vec::new();

        if is_moved {
            // Dimmed row with green checkmark.
            let dim = Style::default().fg(Color::DarkGray);
            let check = Span::styled("\u{2713} ", Style::default().fg(Color::Green));
            spans.push(Span::styled(prefix.to_string(), dim));
            spans.push(Span::styled(format!("{label:<10}"), dim));
            spans.push(Span::styled(format!("{date}  "), dim));
            spans.push(Span::styled(filename.to_string(), dim));
            spans.push(Span::raw("  "));
            spans.push(check);
        } else {
            let style = if is_highlighted {
                Style::default().fg(Color::Yellow)
            } else {
                Style::default()
            };
            spans.push(Span::styled(prefix.to_string(), style));
            spans.push(Span::styled(format!("{label:<10}"), style));
            spans.push(Span::styled(format!("{date}  "), style));
            spans.push(Span::styled(filename.to_string(), style));
        };

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Renders the destination search mode with split pane.
fn render_searching(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let left_area = chunks[0];
    let right_area = chunks[1];

    // Left pane: search results.
    let visible_rows = left_area.height.saturating_sub(1) as usize; // reserve 1 row for input
    let (start, end) = visible_window(app.search_cursor, app.search_results.len(), visible_rows);

    let mut lines: Vec<Line> = Vec::new();
    for i in start..end {
        let result = &app.search_results[i];
        let is_highlighted = i == app.search_cursor;
        let prefix = if is_highlighted { "\u{25b6}" } else { " " };
        let style = if is_highlighted {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(format!("{prefix}{result}"), style)));
    }

    // Input line at the bottom of the left pane.
    lines.push(Line::from(vec![
        Span::styled("> ", Style::default().fg(Color::Cyan)),
        Span::raw(&app.search_query),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));

    let left_paragraph = Paragraph::new(lines);
    frame.render_widget(left_paragraph, left_area);

    // Right pane: files in the highlighted destination.
    render_dest_files(frame, app, right_area);
}

/// Renders the subfolder creation mode (same as search, but with folder name input).
fn render_subfolder_creation(frame: &mut Frame, app: &App, area: Rect) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let left_area = chunks[0];
    let right_area = chunks[1];

    // Left pane: search results (frozen).
    let visible_rows = left_area.height.saturating_sub(1) as usize;
    let (start, end) = visible_window(app.search_cursor, app.search_results.len(), visible_rows);

    let mut lines: Vec<Line> = Vec::new();
    for i in start..end {
        let result = &app.search_results[i];
        let is_highlighted = i == app.search_cursor;
        let prefix = if is_highlighted { "\u{25b6}" } else { " " };
        let style = if is_highlighted {
            Style::default().fg(Color::Yellow)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(format!("{prefix}{result}"), style)));
    }

    // Folder name input line.
    lines.push(Line::from(vec![
        Span::styled("New folder: ", Style::default().fg(Color::Magenta)),
        Span::raw(&app.search_query),
        Span::styled("\u{2588}", Style::default().fg(Color::Magenta)),
    ]));

    let left_paragraph = Paragraph::new(lines);
    frame.render_widget(left_paragraph, left_area);

    render_dest_files(frame, app, right_area);
}

/// Renders the file naming mode.
fn render_naming(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    // Top line: file being filed and chosen destination.
    let filename = app
        .current_file()
        .map(|f| f.filename.as_str())
        .unwrap_or("");
    let dest_rel = app.selected_dest.as_deref().unwrap_or("");
    lines.push(Line::from(vec![
        Span::styled("\u{2192} ", Style::default().fg(Color::Cyan)),
        Span::styled(filename, Style::default().fg(Color::Yellow)),
        Span::styled("  \u{00b7}  ", Style::default().fg(Color::DarkGray)),
        Span::styled(dest_rel, Style::default().fg(Color::Green)),
    ]));

    // Existing files in the destination directory (alphabetical).
    let dest_files = app
        .selected_dest
        .as_deref()
        .and_then(|rel| DestIndex::list_files(&app.dest_root, rel).ok())
        .unwrap_or_default();

    let available_rows = area.height.saturating_sub(2) as usize; // 1 for header, 1 for input
    let skip_count = dest_files.len().saturating_sub(available_rows);
    for name in dest_files.iter().skip(skip_count) {
        lines.push(Line::from(Span::styled(
            format!(" {name}"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Pad to push the input line to the bottom.
    let used_rows = lines.len();
    let total_rows = area.height as usize;
    if used_rows + 1 < total_rows {
        let padding = total_rows - used_rows - 1;
        for _ in 0..padding {
            lines.push(Line::from(""));
        }
    }

    // Input line with extension hint.
    let ext = app
        .current_file()
        .map(|f| {
            std::path::Path::new(&f.filename)
                .extension()
                .map(|e| format!("  [.{}]", e.to_string_lossy()))
                .unwrap_or_default()
        })
        .unwrap_or_default();

    let original_stem = app
        .current_file()
        .map(|f| {
            std::path::Path::new(&f.filename)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    lines.push(Line::from(vec![
        Span::styled("Original: ", Style::default().fg(Color::DarkGray)),
        Span::styled(&original_stem, Style::default().fg(Color::DarkGray)),
        Span::styled(&ext, Style::default().fg(Color::DarkGray)),
        Span::styled("  New name: ", Style::default().fg(Color::Cyan)),
        Span::raw(&app.name_input),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Renders the hint bar at the bottom of the viewport.
fn render_hint_bar(frame: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(ref err) = app.error_message {
        Line::from(Span::styled(err.as_str(), Style::default().fg(Color::Red)))
    } else {
        let hints = match app.state {
            AppState::Browsing => "\u{2191}\u{2193} navigate  Enter file  Space preview  ^C quit",
            AppState::Searching => "Enter confirm  Tab new folder  Esc cancel  ^Spc preview",
            AppState::SubfolderCreation => "Enter create  Esc cancel",
            AppState::Naming => "Enter confirm (empty=keep)  ^U clear  Esc back",
        };
        Line::from(Span::styled(
            hints.to_string(),
            Style::default().fg(Color::DarkGray),
        ))
    };

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

/// Renders file names present in the currently highlighted destination directory.
fn render_dest_files(frame: &mut Frame, app: &App, area: Rect) {
    let selected = app.search_results.get(app.search_cursor);
    let files = selected
        .and_then(|rel| DestIndex::list_files(&app.dest_root, rel).ok())
        .unwrap_or_default();

    let visible_rows = area.height as usize;
    let lines: Vec<Line> = files
        .iter()
        .take(visible_rows)
        .map(|name| {
            Line::from(Span::styled(
                format!("\u{2502} {name}"),
                Style::default().fg(Color::DarkGray),
            ))
        })
        .collect();

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Computes the visible window range for a scrollable list.
///
/// Returns `(start, end)` indices into the list such that the cursor
/// is always visible and the window fits within `visible_rows`.
fn visible_window(cursor: usize, total: usize, visible_rows: usize) -> (usize, usize) {
    if total == 0 || visible_rows == 0 {
        return (0, 0);
    }
    let end_exclusive = total.min(cursor.saturating_sub(visible_rows / 2) + visible_rows);
    let start = end_exclusive.saturating_sub(visible_rows);
    (start, end_exclusive)
}

/// Formats a `SystemTime` as `YYYY-MM-DD`.
fn format_system_time(time: &SystemTime) -> String {
    let datetime: chrono::DateTime<chrono::Local> = (*time).into();
    datetime.format("%Y-%m-%d").to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn visible_window_empty_list() {
        assert_eq!(visible_window(0, 0, 10), (0, 0));
    }

    #[test]
    fn visible_window_fits_entirely() {
        assert_eq!(visible_window(2, 5, 10), (0, 5));
    }

    #[test]
    fn visible_window_scrolled_to_end() {
        let (start, end) = visible_window(99, 100, 10);
        assert_eq!(end, 100);
        assert_eq!(start, 90);
    }

    #[test]
    fn visible_window_cursor_in_middle() {
        let (start, end) = visible_window(50, 100, 10);
        // Cursor at 50, visible_rows 10, half = 5
        // end = min(100, 50-5+10) = min(100, 55) = 55
        // start = 55-10 = 45
        assert_eq!(start, 45);
        assert_eq!(end, 55);
    }

    #[test]
    fn visible_window_cursor_near_start() {
        let (start, end) = visible_window(2, 100, 10);
        // end = min(100, 2-5+10) = min(100, 7) → but 2-5 saturates to 0, so end = min(100, 10) = 10
        // start = 10-10 = 0
        assert_eq!(start, 0);
        assert_eq!(end, 10);
    }

    #[test]
    fn format_system_time_produces_date_string() {
        let now = SystemTime::now();
        let result = format_system_time(&now);
        // Should be YYYY-MM-DD format.
        assert_eq!(result.len(), 10);
        assert_eq!(&result[4..5], "-");
        assert_eq!(&result[7..8], "-");
    }
}
