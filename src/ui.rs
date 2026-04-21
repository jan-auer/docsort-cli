use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
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
        .constraints([
            Constraint::Length(1),
            Constraint::Min(1),
            Constraint::Length(1),
            Constraint::Length(1),
        ])
        .split(area);

    let top_separator_area = chunks[0];
    let content_area = chunks[1];
    let separator_area = chunks[2];
    let hint_area = chunks[3];

    render_separator(frame, top_separator_area);

    match app.state {
        AppState::Browsing => render_browsing(frame, app, content_area),
        AppState::Searching => render_searching(frame, app, content_area),
        AppState::SubfolderCreation => render_subfolder_creation(frame, app, content_area),
        AppState::Naming => render_naming(frame, app, content_area),
    }

    render_separator(frame, separator_area);
    render_hint_bar(frame, app, hint_area);
}

/// Palette of background colors for inbox labels. Cycled by inbox index.
const LABEL_PALETTE: &[Color] = &[Color::Blue, Color::Green, Color::Magenta, Color::Cyan];

/// Builds the ordered list of unique inbox labels from the file list, in
/// first-appearance order.
fn unique_labels(files: &[crate::inbox::InboxFile]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for file in files {
        if !seen.contains(&file.label) {
            seen.push(file.label.clone());
        }
    }
    seen
}

/// Returns the background color for an inbox label by its position in the
/// unique-label list.
fn label_color(label_index: usize) -> Color {
    LABEL_PALETTE[label_index % LABEL_PALETTE.len()]
}

/// Renders the file browser list in Browsing mode.
fn render_browsing(frame: &mut Frame, app: &App, area: Rect) {
    let visible_rows = area.height as usize;
    let (start, end) = visible_window(app.cursor, app.files.len(), visible_rows);

    let labels = unique_labels(&app.files);

    let mut lines: Vec<Line> = Vec::new();
    for i in start..end {
        let file = &app.files[i];
        let is_highlighted = i == app.cursor;
        let is_moved = app.moved.contains_key(&file.path);

        let prefix = if is_highlighted { "\u{25b6} " } else { "  " };
        let label_text = &file.label;
        let filename = &file.filename;

        let label_index = labels.iter().position(|l| *l == file.label).unwrap_or(0);
        let bg = label_color(label_index);

        let mut spans: Vec<Span> = Vec::new();

        if is_moved {
            // Dimmed row with green checkmark.
            let dim = Style::default().fg(Color::DarkGray);
            let check = Span::styled("\u{2713} ", Style::default().fg(Color::Green));
            spans.push(Span::styled(prefix.to_string(), dim));
            spans.push(Span::styled(label_text.to_string(), dim));
            spans.push(Span::raw(" "));
            spans.push(Span::styled(filename.to_string(), dim));
            spans.push(Span::raw("  "));
            spans.push(check);
        } else if is_highlighted {
            let row_style = Style::default().add_modifier(Modifier::BOLD);
            let label_style = Style::default()
                .add_modifier(Modifier::BOLD)
                .fg(Color::White)
                .bg(bg);
            spans.push(Span::styled(prefix.to_string(), row_style));
            spans.push(Span::styled(label_text.to_string(), label_style));
            spans.push(Span::styled(" ".to_string(), row_style));
            spans.push(Span::styled(filename.to_string(), row_style));
        } else {
            let label_style = Style::default().fg(Color::White).bg(bg);
            spans.push(Span::raw(prefix.to_string()));
            spans.push(Span::styled(label_text.to_string(), label_style));
            spans.push(Span::raw(" "));
            spans.push(Span::raw(filename.to_string()));
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
        let prefix = if is_highlighted { "\u{25b6} " } else { "  " };
        let style = if is_highlighted {
            Style::default().add_modifier(Modifier::BOLD)
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
        let prefix = if is_highlighted { "\u{25b6} " } else { "  " };
        let style = if is_highlighted {
            Style::default().add_modifier(Modifier::BOLD)
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

    let available_rows = area.height.saturating_sub(3) as usize; // 1 for header, 2 for input
    let skip_count = dest_files.len().saturating_sub(available_rows);
    for name in dest_files.iter().skip(skip_count) {
        lines.push(Line::from(Span::styled(
            format!(" {name}"),
            Style::default().fg(Color::DarkGray),
        )));
    }

    // Pad to push the two input lines to the bottom.
    let used_rows = lines.len();
    let total_rows = area.height as usize;
    if used_rows + 2 < total_rows {
        let padding = total_rows - used_rows - 2;
        for _ in 0..padding {
            lines.push(Line::from(""));
        }
    }

    let original_stem = app
        .current_file()
        .map(|f| {
            std::path::Path::new(&f.filename)
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_default()
        })
        .unwrap_or_default();

    // Align label values to the same column: pad both labels to the same width.
    let label_original = "Original:";
    let label_new = "New name:";
    let label_width = label_original.len().max(label_new.len());
    let original_label = format!("{:<width$}  ", label_original, width = label_width);
    let new_name_label = format!("{:<width$}  ", label_new, width = label_width);

    lines.push(Line::from(vec![
        Span::styled(original_label, Style::default().fg(Color::DarkGray)),
        Span::styled(&original_stem, Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(vec![
        Span::styled(new_name_label, Style::default().fg(Color::Cyan)),
        Span::raw(&app.name_input),
        Span::styled("\u{2588}", Style::default().fg(Color::Cyan)),
    ]));

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Renders a horizontal separator line filling the given area.
fn render_separator(frame: &mut Frame, area: Rect) {
    let line = "\u{2500}".repeat(area.width as usize);
    let paragraph = Paragraph::new(Line::from(Span::styled(
        line,
        Style::default().fg(HINT_COLOR),
    )));
    frame.render_widget(paragraph, area);
}

/// Muted mid-grey used for hint bar text and separators.
const HINT_COLOR: Color = Color::Rgb(128, 128, 128);

/// Renders the hint bar at the bottom of the viewport.
fn render_hint_bar(frame: &mut Frame, app: &App, area: Rect) {
    let line = if let Some(ref err) = app.error_message {
        let color = if app.ctrl_c_hint {
            HINT_COLOR
        } else {
            Color::Red
        };
        Line::from(Span::styled(err.as_str(), Style::default().fg(color)))
    } else {
        let hints = match app.state {
            AppState::Browsing => "\u{2191}\u{2193} navigate  Enter file  Space preview  ^C quit",
            AppState::Searching => "Enter confirm  Tab new folder  Esc cancel  ^Spc preview",
            AppState::SubfolderCreation => "Enter create  Esc cancel",
            AppState::Naming => "Enter confirm (empty=keep)  ^U clear  Esc back",
        };
        Line::from(Span::styled(
            hints.to_string(),
            Style::default().fg(HINT_COLOR),
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
}
