use ratatui::layout::{Constraint, Direction, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Padding, Paragraph};
use ratatui::Frame;

use crate::app::{App, AppState};
use crate::dest::DestIndex;
use crate::inbox::InboxFile;

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
        AppState::Browsing | AppState::ConfirmDelete | AppState::ConfirmOverwrite => {
            render_browsing(frame, app, content_area)
        }
        AppState::Searching => render_searching(frame, app, content_area),
        AppState::SubfolderCreation => render_subfolder_creation(frame, app, content_area),
        AppState::Naming => render_naming(frame, app, content_area),
    }

    render_separator(frame, separator_area);
    render_hint_bar(frame, app, hint_area);
}

/// Palette of foreground colors for inbox labels. Cycled by inbox index.
const LABEL_PALETTE: &[Color] = &[Color::Blue, Color::Green, Color::Magenta, Color::Cyan];

/// Builds the ordered list of unique inbox labels from the file list, in
/// first-appearance order.
fn unique_labels(files: &[InboxFile]) -> Vec<String> {
    let mut seen: Vec<String> = Vec::new();
    for file in files {
        if !seen.contains(&file.label) {
            seen.push(file.label.clone());
        }
    }
    seen
}

/// Returns the foreground color for an inbox label by its position in the
/// unique-label list.
fn label_color(label_index: usize) -> Color {
    LABEL_PALETTE[label_index % LABEL_PALETTE.len()]
}

/// Renders the file browser list in Browsing mode.
fn render_browsing(frame: &mut Frame, app: &App, area: Rect) {
    let visible_rows = area.height as usize;
    let (start, end) = visible_window(app.cursor, app.files.len(), visible_rows);

    let labels = unique_labels(&app.files);
    let max_label_len = labels.iter().map(|l| l.len()).max().unwrap_or(0);

    // Fixed per-row overhead: prefix (2) + padded_label + " ❯ " (3).
    let fixed_overhead = 2 + max_label_len + 3;
    let total_width = area.width as usize;

    let mut lines: Vec<Line> = Vec::new();
    for i in start..end {
        let file = &app.files[i];
        let is_highlighted = i == app.cursor;
        let is_moved = app.moved.contains_key(&file.path);

        let prefix = if is_highlighted { "\u{25b6} " } else { "  " };

        let label_index = labels.iter().position(|l| *l == file.label).unwrap_or(0);
        let fg = label_color(label_index);

        // Pad the label to max_label_len so chevrons and filenames stay aligned.
        let padded_label = format!("{:<width$}", file.label, width = max_label_len);
        // Chevron with leading space and trailing space, rendered in the label colour.
        let label_chevron = format!("{padded_label} \u{276f} ");

        // For moved rows, "  ✓ " (4 chars) is appended after the filename.
        let moved_suffix_len = if is_moved { 4 } else { 0 };
        let budget = total_width.saturating_sub(fixed_overhead + moved_suffix_len);

        let (subfolder_display, filename_display) =
            truncate_file_row(file.subfolder.as_deref(), &file.filename, budget);

        let mut spans: Vec<Span> = Vec::new();

        if is_moved {
            // Dimmed row with green checkmark.
            let dim = Style::default().fg(HINT_COLOR);
            let check = Span::styled("\u{2713} ", Style::default().fg(Color::Green));
            spans.push(Span::styled(prefix.to_string(), dim));
            spans.push(Span::styled(label_chevron, dim));
            if !subfolder_display.is_empty() {
                spans.push(Span::styled(subfolder_display, dim));
            }
            spans.push(Span::styled(filename_display, dim));
            spans.push(Span::raw("  "));
            spans.push(check);
        } else if is_highlighted {
            let row_style = Style::default().add_modifier(Modifier::BOLD);
            let label_style = Style::default().add_modifier(Modifier::BOLD).fg(fg);
            let subfolder_style = Style::default().add_modifier(Modifier::BOLD).fg(HINT_COLOR);
            spans.push(Span::styled(prefix.to_string(), row_style));
            spans.push(Span::styled(label_chevron, label_style));
            if !subfolder_display.is_empty() {
                spans.push(Span::styled(subfolder_display, subfolder_style));
            }
            spans.push(Span::styled(filename_display, row_style));
        } else {
            let label_style = Style::default().fg(fg);
            let subfolder_style = Style::default().fg(HINT_COLOR);
            spans.push(Span::raw(prefix.to_string()));
            spans.push(Span::styled(label_chevron, label_style));
            if !subfolder_display.is_empty() {
                spans.push(Span::styled(subfolder_display, subfolder_style));
            }
            spans.push(Span::raw(filename_display));
        };

        lines.push(Line::from(spans));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, area);
}

/// Truncates a file row to fit within `budget` characters, returning
/// `(subfolder_display, filename_display)`.
///
/// When `subfolder` is `None` or empty, the subfolder display is always `""`.
/// Truncation proceeds in two steps: first the subfolder is shortened, then
/// (as a last resort) the filename stem is middle-truncated.
pub fn truncate_file_row(
    subfolder: Option<&str>,
    filename: &str,
    budget: usize,
) -> (String, String) {
    let subfolder = match subfolder {
        Some(s) if !s.is_empty() => s,
        _ => return (String::new(), filename.to_string()),
    };

    // Step 1: fits as-is?
    if subfolder.chars().count() + filename.chars().count() <= budget {
        return (subfolder.to_string(), filename.to_string());
    }

    // Step 2: truncate subfolder to make room for the full filename.
    let subfolder_budget = budget.saturating_sub(filename.chars().count());
    let truncated_subfolder = truncate_subfolder(subfolder, subfolder_budget);

    if truncated_subfolder.chars().count() + filename.chars().count() <= budget {
        return (truncated_subfolder, filename.to_string());
    }

    // Step 3: last resort — truncate the filename stem.
    let filename_budget = budget.saturating_sub(truncated_subfolder.chars().count());
    let truncated_filename = truncate_filename(filename, filename_budget);
    (truncated_subfolder, truncated_filename)
}

/// Truncates a subfolder string (e.g. `"ProjectA/Client Work/"`) to fit within
/// `budget` characters, using middle-ellipsis strategies.
///
/// Returns an empty string when the subfolder cannot be represented at all.
fn truncate_subfolder(subfolder: &str, budget: usize) -> String {
    // Drop trailing slash before splitting, add it back in each candidate.
    let without_slash = subfolder.trim_end_matches('/');
    let segments: Vec<&str> = without_slash.split('/').filter(|s| !s.is_empty()).collect();

    if segments.is_empty() {
        return String::new();
    }

    if segments.len() == 1 {
        return truncate_single_segment(segments[0], budget);
    }

    // Multiple segments: try candidates from most to least informative.
    let first = segments[0];
    let last = *segments.last().expect("segments non-empty");

    // Candidate: "first/…/last/"
    // Require at least 3 visible chars on each side of the ellipsis (total 6 non-slash chars)
    // so hints like "A/…/B/" with too little visible content on either side are rejected.
    let candidate_full = format!("{first}/\u{2026}/{last}/");
    if candidate_full.chars().count() <= budget && visible_content_len(&candidate_full) >= 6 {
        return candidate_full;
    }

    // Candidate: "first/…/"
    let candidate_first = format!("{first}/\u{2026}/");
    if candidate_first.chars().count() <= budget && visible_content_len(&candidate_first) >= 6 {
        return candidate_first;
    }

    // Candidate: "…/"
    let candidate_ellipsis = "\u{2026}/";
    if candidate_ellipsis.chars().count() <= budget {
        return candidate_ellipsis.to_string();
    }

    String::new()
}

/// Truncates a single path segment to fit within `budget` characters,
/// including the trailing `/`.
///
/// Minimum result: `"…/"` (3 bytes for the `…` + 1 for `/`). If even that
/// doesn't fit, returns `""`.
fn truncate_single_segment(segment: &str, budget: usize) -> String {
    // Trailing slash is always included.
    let full = format!("{segment}/");
    if full.chars().count() <= budget {
        return full;
    }

    // Minimum meaningful: 3 front + … + 3 back + /  = 8 chars (7 visible before /).
    // We need at least budget >= 8 to attempt middle truncation.
    // "…/" is 2 chars (… is 1 char, / is 1 char), minimum display.
    let ellipsis = "\u{2026}";
    let ellipsis_slash = "\u{2026}/";

    if budget < ellipsis_slash.chars().count() {
        return String::new();
    }

    // Available for content chars (not counting … and /).
    // We need: front + … + back + / <= budget
    // front + back = budget - ellipsis.chars().count() - 1 (for /)
    let available_for_content = budget.saturating_sub(ellipsis.chars().count() + 1);

    // Minimum: 3 chars on each side = 6 content chars.
    if available_for_content < 6 {
        // Can't meet the 3+3 minimum — use "…/" only.
        return ellipsis_slash.to_string();
    }

    let chars: Vec<char> = segment.chars().collect();
    let front_len = available_for_content / 2;
    let back_len = available_for_content - front_len;

    // If the segment is short enough with the splits, just use it.
    if front_len + back_len >= chars.len() {
        return full;
    }

    let front: String = chars[..front_len].iter().collect();
    let back: String = chars[chars.len() - back_len..].iter().collect();
    format!("{front}{ellipsis}{back}/")
}

/// Returns the number of visible (non-slash) characters in a truncated
/// subfolder string, used to enforce the 6-char minimum content rule.
fn visible_content_len(s: &str) -> usize {
    s.chars().filter(|&c| c != '/').count()
}

/// Truncates a filename to fit within `budget` characters, preserving the
/// extension and using middle-ellipsis on the stem.
fn truncate_filename(filename: &str, budget: usize) -> String {
    if filename.chars().count() <= budget {
        return filename.to_string();
    }

    let path = std::path::Path::new(filename);
    let ext = path
        .extension()
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();
    let stem = path
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| filename.to_string());

    let ellipsis = "\u{2026}";

    // Budget for stem + "…" = budget - ext.chars().count() - ellipsis.chars().count()
    // We need at least 2 chars of stem visible (1 front + 1 back minimum... but
    // spec says 2 chars + …, so n >= 4 means at least 2 stem chars + …):
    let stem_budget = budget.saturating_sub(ext.chars().count() + ellipsis.chars().count());

    if stem_budget >= 2 {
        // Place … in middle: floor(n/2) from start, ceil(n/2) from end.
        let stem_chars: Vec<char> = stem.chars().collect();
        let front_len = stem_budget / 2;
        let back_len = stem_budget - front_len;

        if front_len + back_len >= stem_chars.len() {
            // Stem fits without truncation; just ensure we're within budget.
            let full = format!("{stem}{ext}");
            if full.chars().count() <= budget {
                return full;
            }
        }

        let front: String = stem_chars[..front_len.min(stem_chars.len())]
            .iter()
            .collect();
        let back_start = stem_chars.len().saturating_sub(back_len);
        let back: String = stem_chars[back_start..].iter().collect();
        return format!("{front}{ellipsis}{back}{ext}");
    }

    // Not enough room for stem middle-truncation — truncate to whatever fits with trailing ….
    if budget >= ellipsis.len() {
        let chars: Vec<char> = filename.chars().collect();
        let keep = budget - ellipsis.len();
        let truncated: String = chars[..keep.min(chars.len())].iter().collect();
        return format!("{truncated}{ellipsis}");
    }

    // Degenerate: extremely narrow budget.
    filename.chars().take(budget).collect()
}

/// Renders the destination search mode with split pane.
fn render_searching(frame: &mut Frame, app: &App, area: Rect) {
    render_search_pane(frame, app, area, "> ", Color::Cyan, "type to search");
}

/// Renders the subfolder creation mode (same layout as search, but with folder name input).
fn render_subfolder_creation(frame: &mut Frame, app: &App, area: Rect) {
    render_search_pane(
        frame,
        app,
        area,
        "New folder: ",
        Color::Magenta,
        "type folder name",
    );
}

/// Shared split-pane renderer used by both search and subfolder-creation modes.
///
/// Renders a 60/40 horizontal split: the left pane shows the search results
/// list with a text input at the bottom; the right pane shows the files inside
/// the highlighted destination. The `prefix` label and its `prefix_color` are
/// shown before the cursor in the input line; `placeholder` is shown when the
/// query is empty.
fn render_search_pane(
    frame: &mut Frame,
    app: &App,
    area: Rect,
    prefix: &str,
    prefix_color: Color,
    placeholder: &str,
) {
    let chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([Constraint::Percentage(60), Constraint::Percentage(40)])
        .split(area);

    let left_area = chunks[0];
    let right_area = chunks[1];

    // Left pane: split vertically into results (top) and input (bottom).
    let left_block = Block::default().padding(Padding::new(0, 1, 0, 0));
    let left_inner = left_block.inner(left_area);

    let left_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Min(0), Constraint::Length(1)])
        .split(left_inner);

    let results_area = left_chunks[0];
    let input_area = left_chunks[1];

    let visible_rows = results_area.height as usize;
    let (start, end) = visible_window(app.search_cursor, app.search_results.len(), visible_rows);

    let mut lines: Vec<Line> = Vec::new();
    for i in start..end {
        let result = &app.search_results[i];
        let is_highlighted = i == app.search_cursor;
        let row_prefix = if is_highlighted { "\u{25b6} " } else { "  " };
        let style = if is_highlighted {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
        };
        lines.push(Line::from(Span::styled(
            format!("{row_prefix}{result}"),
            style,
        )));
    }

    let results_paragraph = Paragraph::new(lines);
    frame.render_widget(results_paragraph, results_area);

    // Input line at the bottom of the left pane.
    let input_spans = if app.search_query.is_empty() {
        vec![
            Span::styled(prefix.to_string(), Style::default().fg(prefix_color)),
            Span::styled(placeholder, Style::default().fg(HINT_COLOR)),
        ]
    } else {
        let before: String = app
            .search_query
            .chars()
            .take(app.search_input_pos)
            .collect();
        let cursor_char: Option<char> = app.search_query.chars().nth(app.search_input_pos);
        let after: String = app
            .search_query
            .chars()
            .skip(app.search_input_pos + 1)
            .collect();
        let mut spans = vec![Span::styled(
            prefix.to_string(),
            Style::default().fg(prefix_color),
        )];
        spans.push(Span::raw(before));
        if let Some(ch) = cursor_char {
            spans.push(Span::styled(
                ch.to_string(),
                Style::default().add_modifier(Modifier::REVERSED),
            ));
        } else {
            spans.push(Span::styled(
                " ",
                Style::default().add_modifier(Modifier::REVERSED),
            ));
        }
        spans.push(Span::raw(after));
        spans
    };

    let input_paragraph = Paragraph::new(Line::from(input_spans));
    frame.render_widget(input_paragraph, input_area);

    // Right pane: files in the highlighted destination.
    render_dest_files(frame, app, right_area);
}

/// Renders the file naming mode.
fn render_naming(frame: &mut Frame, app: &App, area: Rect) {
    let mut lines: Vec<Line> = Vec::new();

    // Top line: destination folder.
    let dest_rel = app.selected_dest.as_deref().unwrap_or("");
    lines.push(Line::from(vec![
        Span::styled("\u{2192} ", Style::default().fg(Color::Cyan)),
        Span::styled(dest_rel, Style::default().add_modifier(Modifier::BOLD)),
    ]));

    // Existing files in the destination directory (alphabetical), excluding
    // the file currently being filed (it isn't there yet).
    let current_filename = app
        .current_file()
        .map(|f| f.filename.as_str())
        .unwrap_or("");
    let dest_files: Vec<String> = app
        .selected_dest
        .as_deref()
        .and_then(|rel| DestIndex::list_files(&app.dest_root, rel).ok())
        .unwrap_or_default()
        .into_iter()
        .filter(|name| name.as_str() != current_filename)
        .collect();

    let available_rows = area.height.saturating_sub(3) as usize; // 1 for header, 2 for input
    let skip_count = dest_files.len().saturating_sub(available_rows);
    let visible_files = &dest_files[skip_count..];

    if visible_files.is_empty() {
        // Render placeholder when no files are present.
        lines.push(Line::from(Span::styled(
            "  \u{2514}\u{2500} (no files)",
            Style::default().fg(HINT_COLOR),
        )));
    } else {
        let last_idx = visible_files.len().saturating_sub(1);
        for (i, name) in visible_files.iter().enumerate() {
            let tree_symbol = if i == last_idx {
                "  \u{2514}\u{2500} "
            } else {
                "  \u{251c}\u{2500} "
            };
            lines.push(Line::from(vec![
                Span::styled(tree_symbol, Style::default().fg(HINT_COLOR)),
                Span::raw(name.as_str()),
            ]));
        }
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

    let ext_suffix = app
        .current_file()
        .and_then(|f| std::path::Path::new(&f.filename).extension())
        .map(|e| format!(".{}", e.to_string_lossy()))
        .unwrap_or_default();

    lines.push(Line::from(vec![
        Span::styled(original_label, Style::default()),
        Span::styled(&original_stem, Style::default()),
        Span::styled(&ext_suffix, Style::default().fg(HINT_COLOR)),
    ]));

    let name_before: String = app.name_input.chars().take(app.name_input_pos).collect();
    let cursor_char: Option<char> = app.name_input.chars().nth(app.name_input_pos);
    let name_after: String = app
        .name_input
        .chars()
        .skip(app.name_input_pos + 1)
        .collect();
    let mut new_name_spans = vec![Span::styled(
        new_name_label,
        Style::default().fg(Color::Cyan),
    )];
    new_name_spans.push(Span::raw(name_before));
    if let Some(ch) = cursor_char {
        new_name_spans.push(Span::styled(
            ch.to_string(),
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        new_name_spans.push(Span::raw(name_after));
        if !ext_suffix.is_empty() {
            new_name_spans.push(Span::styled(&ext_suffix, Style::default().fg(HINT_COLOR)));
        }
    } else if app.name_input.is_empty() {
        // Cursor at end, input empty: reversed-space cursor, then <keep> hint if extension exists.
        new_name_spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        if !ext_suffix.is_empty() {
            new_name_spans.push(Span::styled("<keep>", Style::default().fg(HINT_COLOR)));
        }
    } else if !ext_suffix.is_empty() {
        // Cursor at end of non-empty input with extension: use '.' as the cursor, rest of ext in hint color.
        let dot_cursor = ".";
        let ext_rest = &ext_suffix[dot_cursor.len()..];
        new_name_spans.push(Span::styled(
            dot_cursor,
            Style::default().add_modifier(Modifier::REVERSED),
        ));
        new_name_spans.push(Span::styled(ext_rest, Style::default().fg(HINT_COLOR)));
    } else {
        // Cursor at end, no extension: reversed-space cursor.
        new_name_spans.push(Span::styled(
            " ",
            Style::default().add_modifier(Modifier::REVERSED),
        ));
    }
    lines.push(Line::from(new_name_spans));

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

/// Muted mid-grey used for hints, separators, and decorative elements.
const HINT_COLOR: Color = Color::Rgb(140, 140, 140);

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
        match app.state {
            AppState::Browsing => {
                let text = if app.current_file_is_moved() {
                    "\u{2191}\u{2193} navigate  \u{21b5} file  u undo  r refresh  Space preview  o open  f finder  ^C quit"
                } else {
                    "\u{2191}\u{2193} navigate  \u{21b5} file  d delete  r refresh  Space preview  o open  f finder  ^C quit"
                };
                Line::from(Span::styled(text, Style::default().fg(HINT_COLOR)))
            }
            AppState::ConfirmDelete => Line::from(Span::styled(
                "delete? [y/N]",
                Style::default().fg(Color::Red),
            )),
            AppState::ConfirmOverwrite => Line::from(Span::styled(
                "file already exists at destination — overwrite? [y/N]",
                Style::default().fg(Color::Rgb(210, 120, 0)),
            )),
            AppState::Searching => Line::from(Span::styled(
                "\u{21b5} confirm  Tab new folder  Esc cancel  ^Spc preview",
                Style::default().fg(HINT_COLOR),
            )),
            AppState::SubfolderCreation => Line::from(Span::styled(
                "\u{21b5} create  Esc cancel  ^Spc preview",
                Style::default().fg(HINT_COLOR),
            )),
            AppState::Naming => Line::from(Span::styled(
                "\u{21b5} confirm (empty=keep)  Esc clear/back",
                Style::default().fg(HINT_COLOR),
            )),
        }
    };

    let paragraph = Paragraph::new(line);
    frame.render_widget(paragraph, area);
}

/// Renders file names present in the currently highlighted destination directory.
///
/// The pane always fills its full allocated height; rows below the last file
/// are left blank. File names use the same style as non-highlighted entries in
/// the left fuzzy results list.
fn render_dest_files(frame: &mut Frame, app: &App, area: Rect) {
    let block = Block::default()
        .borders(Borders::LEFT)
        .border_style(Style::default().fg(HINT_COLOR))
        .padding(Padding::new(1, 0, 0, 0));
    let inner = block.inner(area);
    frame.render_widget(block, area);

    let selected = app.search_results.get(app.search_cursor);
    let files = selected
        .and_then(|rel| DestIndex::list_files(&app.dest_root, rel).ok())
        .unwrap_or_default();

    let visible_rows = inner.height as usize;
    let mut lines: Vec<Line> = if files.is_empty() {
        vec![Line::from(Span::styled(
            "(no files)",
            Style::default().fg(HINT_COLOR),
        ))]
    } else {
        files
            .iter()
            .take(visible_rows)
            .map(|name| Line::from(name.as_str()))
            .collect()
    };

    // Pad remaining rows with empty lines.
    while lines.len() < visible_rows {
        lines.push(Line::from(""));
    }

    let paragraph = Paragraph::new(lines);
    frame.render_widget(paragraph, inner);
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

    // ── truncate_file_row ────────────────────────────────────────────────────

    #[test]
    fn truncate_fits_no_subfolder() {
        let (sub, name) = truncate_file_row(None, "report.pdf", 100);
        assert_eq!(sub, "");
        assert_eq!(name, "report.pdf");
    }

    #[test]
    fn truncate_fits_with_subfolder() {
        let (sub, name) = truncate_file_row(Some("ProjectA/"), "report.pdf", 100);
        assert_eq!(sub, "ProjectA/");
        assert_eq!(name, "report.pdf");
    }

    #[test]
    fn truncate_empty_subfolder_treated_as_none() {
        let (sub, name) = truncate_file_row(Some(""), "report.pdf", 100);
        assert_eq!(sub, "");
        assert_eq!(name, "report.pdf");
    }

    #[test]
    fn truncate_subfolder_multi_segment_first_ellipsis_last() {
        // "ProjectA/…/ClientWork/" fits within a reasonable budget.
        // "ProjectA/ClientWork/Invoices/" is 30 chars; filename "report.pdf" is 10.
        // Total = 40. Budget = 35 forces subfolder truncation.
        let subfolder = "ProjectA/ClientWork/Invoices/";
        let filename = "report.pdf";
        let budget = 35; // 35 - 10 = 25 for subfolder
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(name, filename);
        // "ProjectA/…/Invoices/" = 20 chars — fits in 25
        assert_eq!(sub, "ProjectA/\u{2026}/Invoices/");
    }

    #[test]
    fn truncate_subfolder_multi_segment_first_ellipsis_only() {
        // Budget forces dropping the last segment too.
        // "ProjectA/ClientWork/Invoices/" subfolder + "report.pdf" filename.
        // subfolder_budget = 15, "ProjectA/…/Invoices/" = 20 chars doesn't fit.
        // "ProjectA/…/" = 12 chars fits.
        let subfolder = "ProjectA/ClientWork/Invoices/";
        let filename = "report.pdf";
        let budget = 25; // 25 - 10 = 15 for subfolder
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(name, filename);
        assert_eq!(sub, "ProjectA/\u{2026}/");
    }

    #[test]
    fn truncate_subfolder_multi_segment_ellipsis_slash_only() {
        // Budget so tight only "…/" fits for subfolder.
        // subfolder_budget = 3 — "ProjectA/…/" doesn't fit, "…/" (2 chars) doesn't fit either.
        // subfolder_budget = 4 — "…/" (2 chars) fits.
        let subfolder = "ProjectA/ClientWork/";
        let filename = "report.pdf"; // 10 chars
                                     // budget = 14 → subfolder_budget = 4, "…/" fits
        let budget = 14;
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(name, filename);
        assert_eq!(sub, "\u{2026}/");
    }

    #[test]
    fn truncate_subfolder_single_segment_middle_truncated() {
        // Single segment "VeryLongFolderName/" — budget forces middle truncation.
        // "VeryLongFolderName/" = 19 chars; filename "doc.pdf" = 7 chars; total = 26.
        // Budget = 20 → subfolder_budget = 13.
        // available_for_content = 13 - 1 (… = 1 char) - 1 (/) = 11 >= 6, so middle truncation.
        // front = 11/2 = 5, back = 6. "VeryL…erName/" = 13 chars.
        let subfolder = "VeryLongFolderName/";
        let filename = "doc.pdf";
        let budget = 20;
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(name, filename);
        // front=5: "VeryL", back=6: "erName" (last 6 of "VeryLongFolderName")
        assert_eq!(sub, "VeryL\u{2026}erName/");
    }

    #[test]
    fn truncate_subfolder_single_segment_falls_back_to_ellipsis_slash() {
        // Budget so small that single-segment middle-truncation can't meet 3+3 minimum.
        // subfolder_budget = 7: available_for_content = 7-1(…)-1(/) = 5 < 6, so "…/".
        let subfolder = "LongName/";
        let filename = "doc.pdf"; // 7 chars
        let budget = 14; // subfolder_budget = 7
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(name, filename);
        assert_eq!(sub, "\u{2026}/");
    }

    #[test]
    fn truncate_filename_last_resort() {
        // When subfolder can't be reduced further and filename still overflows.
        // Use a 1-segment subfolder "A/" that can't be reduced (already minimal).
        // "…/" (4) + filename must fit in budget.
        // filename = "VeryLongDocumentName.pdf" (24 chars); budget = 20.
        // subfolder "A/" = 2 chars; 2 + 24 = 26 > 20.
        // subfolder_budget = 20 - 24 = 0 → truncated_subfolder = "".
        // filename_budget = 20 - 0 = 20; filename truncation.
        let subfolder = "A/";
        let filename = "VeryLongDocumentName.pdf";
        let budget = 20;
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        // subfolder dropped (budget 0), filename truncated to 20 chars by char count
        assert_eq!(sub, "");
        assert!(
            name.chars().count() <= 20,
            "name too long: {name:?} ({} chars)",
            name.chars().count()
        );
        assert!(
            name.contains('\u{2026}'),
            "expected ellipsis in name: {name:?}"
        );
    }

    #[test]
    fn truncate_very_narrow_budget() {
        let (sub, name) = truncate_file_row(Some("Deep/Path/"), "file.txt", 3);
        // With budget=3, subfolder_budget = 3-8 = 0, so subfolder dropped.
        // filename_budget = 3, truncate to 3 chars.
        assert_eq!(sub, "");
        assert!(name.chars().count() <= 3);
    }

    #[test]
    fn truncate_budget_exactly_fits() {
        let subfolder = "Proj/";
        let filename = "doc.pdf";
        let budget = subfolder.len() + filename.len(); // exact fit
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(sub, subfolder);
        assert_eq!(name, filename);
    }

    #[test]
    fn truncate_non_ascii_subfolder_no_spurious_truncation() {
        // "Ré/seau/Réseau/" has multi-byte chars — .len() would overcount.
        // char count: "Réseau/" = 7 chars, filename "rapport.pdf" = 11 chars, total = 18 chars.
        // budget = 18 (exact fit by char count) — must not truncate.
        let subfolder = "Réseau/";
        let filename = "rapport.pdf";
        let budget = subfolder.chars().count() + filename.chars().count(); // 7 + 11 = 18
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(
            sub, subfolder,
            "non-ASCII subfolder should not be truncated when it fits by char count"
        );
        assert_eq!(name, filename);
    }

    #[test]
    fn truncate_cjk_subfolder_no_spurious_truncation() {
        // CJK characters are 3 bytes each in UTF-8.
        // "文書/" = 3 chars (3 CJK + 1 slash), filename "doc.pdf" = 7 chars, total = 11 chars.
        // budget = 11 — must not truncate.
        let subfolder = "文書/";
        let filename = "doc.pdf";
        let budget = subfolder.chars().count() + filename.chars().count(); // 3 + 7 = 10
        let (sub, name) = truncate_file_row(Some(subfolder), filename, budget);
        assert_eq!(
            sub, subfolder,
            "CJK subfolder should not be truncated when it fits by char count"
        );
        assert_eq!(name, filename);
    }

    // ── visible_window ───────────────────────────────────────────────────────

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
