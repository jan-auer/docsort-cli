use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

use crate::config::Config;
use crate::dest::DestIndex;
use crate::file_ops;
use crate::inbox::InboxFile;
use crate::quicklook::QuickLook;

/// Maximum number of visible rows in the inline TUI viewport.
pub const MAX_LIST_HEIGHT: u16 = 15;

/// Minimum number of visible rows in the inline TUI viewport.
const MIN_VIEWPORT_HEIGHT: u16 = 4;

/// Computes the inline viewport height for a given number of inbox files.
///
/// Returns `min(MAX_LIST_HEIGHT, max(MIN_VIEWPORT_HEIGHT, files + 3))` where
/// the `+3` accounts for one top separator, one bottom separator, and one hint
/// bar line.
pub fn viewport_height(file_count: usize) -> u16 {
    let needed = (file_count as u16).saturating_add(3);
    needed.clamp(MIN_VIEWPORT_HEIGHT, MAX_LIST_HEIGHT)
}

/// The four modes of the application state machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppState {
    /// Browsing the file list.
    Browsing,
    /// Searching for a destination directory.
    Searching,
    /// Creating a new subfolder inside the selected destination.
    SubfolderCreation,
    /// Entering a new filename for the file being filed.
    Naming,
    /// Waiting for the user to confirm deletion of the highlighted file.
    ConfirmDelete,
}

/// Actions returned by `App::handle_event` to communicate with the event loop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AppAction {
    /// No special action needed; continue the event loop.
    Continue,
    /// A file was successfully moved; the event loop must tear down and
    /// reinitialize the terminal to flush a summary line.
    CommitMove {
        /// Original inbox path of the moved file.
        src: PathBuf,
        /// Final destination path after the move.
        dest: PathBuf,
        /// Filename used at the destination.
        name: String,
    },
    /// A file was deleted from disk; the event loop must tear down and
    /// reinitialize the terminal to flush a summary line.
    DeleteFile {
        /// Path of the deleted file.
        path: PathBuf,
    },
    /// The user requested to quit the application.
    Quit,
}

/// Maximum number of destination history entries kept in memory.
const MAX_DEST_HISTORY: usize = 20;

/// Core application state driving the TUI.
#[derive(Debug)]
pub struct App {
    /// Combined inbox file list, newest first.
    pub files: Vec<InboxFile>,
    /// Index into `files` for the highlighted row.
    pub cursor: usize,
    /// Records src → dest for files moved during this session.
    pub moved: HashMap<PathBuf, PathBuf>,
    /// Fuzzy-searchable index of destination subdirectories.
    pub dest_index: DestIndex,
    /// Quick Look preview manager.
    pub quick_look: QuickLook,
    /// Current state machine mode.
    pub state: AppState,
    /// Live search/filter text in Searching and SubfolderCreation modes.
    pub search_query: String,
    /// Current fuzzy search results (relative directory paths).
    pub search_results: Vec<String>,
    /// Index into `search_results` for the highlighted result.
    pub search_cursor: usize,
    /// Text in the Naming mode input field.
    pub name_input: String,
    /// Cursor position (char index) within `search_query`.
    pub search_input_pos: usize,
    /// Cursor position (char index) within `name_input`.
    pub name_input_pos: usize,
    /// Timestamp of the last Ctrl+C press, for double-tap quit.
    pub last_ctrl_c: Option<Instant>,
    /// Error message to display in the hint bar.
    pub error_message: Option<String>,
    /// When `true`, the hint bar message is a soft hint (rendered in gray)
    /// rather than a real error (rendered in red).
    pub ctrl_c_hint: bool,
    /// Root path of the destination archive.
    pub dest_root: PathBuf,
    /// The destination path chosen in Searching mode, carried into Naming.
    pub selected_dest: Option<String>,
    /// In-session history of recently-used destination folders, most recent first.
    pub dest_history: Vec<String>,
}

impl App {
    /// Creates a new `App` from scanned inbox files, a destination index, and config.
    pub fn new(files: Vec<InboxFile>, dest_index: DestIndex, config: &Config) -> Self {
        Self {
            files,
            cursor: 0,
            moved: HashMap::new(),
            dest_index,
            quick_look: QuickLook::new(),
            state: AppState::Browsing,
            search_query: String::new(),
            search_results: Vec::new(),
            search_cursor: 0,
            name_input: String::new(),
            search_input_pos: 0,
            name_input_pos: 0,
            last_ctrl_c: None,
            error_message: None,
            ctrl_c_hint: false,
            dest_root: config.destination.root.clone(),
            selected_dest: None,
            dest_history: Vec::new(),
        }
    }

    /// Called on every event-loop tick (whether or not a key arrived).
    ///
    /// Clears the Ctrl+C hint and error message once the 1-second window expires.
    pub fn tick(&mut self) {
        let hint_expired = self
            .last_ctrl_c
            .map(|t| t.elapsed() >= Duration::from_secs(1))
            .unwrap_or(false);
        if hint_expired {
            self.last_ctrl_c = None;
            self.error_message = None;
            self.ctrl_c_hint = false;
        }
    }

    /// Processes a single key event and returns the action the event loop should take.
    pub fn handle_event(&mut self, event: KeyEvent) -> AppAction {
        // Only handle key press events.
        if event.kind != KeyEventKind::Press {
            return AppAction::Continue;
        }

        // Double Ctrl+C quit logic applies in all modes.
        if event.code == KeyCode::Char('c') && event.modifiers.contains(KeyModifiers::CONTROL) {
            return self.handle_ctrl_c();
        }

        // Clear error on any non-Ctrl+C key press.
        self.error_message = None;
        // Reset Ctrl+C timer on any other key press.
        self.last_ctrl_c = None;

        match self.state {
            AppState::Browsing => self.handle_browsing(event),
            AppState::Searching => self.handle_searching(event),
            AppState::SubfolderCreation => self.handle_subfolder_creation(event),
            AppState::Naming => self.handle_naming(event),
            AppState::ConfirmDelete => self.handle_confirm_delete(event),
        }
    }

    /// Handles the double Ctrl+C quit pattern.
    fn handle_ctrl_c(&mut self) -> AppAction {
        if let Some(last) = self.last_ctrl_c {
            if last.elapsed().as_millis() < 1000 {
                return AppAction::Quit;
            }
        }
        self.last_ctrl_c = Some(Instant::now());
        self.error_message = Some("Press Ctrl+C again to quit".to_string());
        self.ctrl_c_hint = true;
        AppAction::Continue
    }

    /// Handles key events in Browsing mode.
    fn handle_browsing(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Up => {
                self.move_cursor_up();
                AppAction::Continue
            }
            KeyCode::Down => {
                self.move_cursor_down();
                AppAction::Continue
            }
            KeyCode::Enter => {
                self.enter_searching();
                AppAction::Continue
            }
            KeyCode::Char(' ') => {
                self.toggle_quick_look();
                AppAction::Continue
            }
            KeyCode::Char('o') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.open_highlighted_file();
                AppAction::Continue
            }
            KeyCode::Char('r') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.reveal_highlighted_file_in_finder();
                AppAction::Continue
            }
            KeyCode::Char('o') => {
                self.open_last_dest_in_finder();
                AppAction::Continue
            }
            KeyCode::Char('d') => {
                self.enter_confirm_delete();
                AppAction::Continue
            }
            _ => AppAction::Continue,
        }
    }

    /// Handles key events in Searching mode.
    fn handle_searching(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Up => {
                self.move_search_cursor_up();
                AppAction::Continue
            }
            KeyCode::Down => {
                self.move_search_cursor_down();
                AppAction::Continue
            }
            KeyCode::Left => {
                if self.search_input_pos > 0 {
                    self.search_input_pos -= 1;
                }
                AppAction::Continue
            }
            KeyCode::Right => {
                let char_count = self.search_query.chars().count();
                if self.search_input_pos < char_count {
                    self.search_input_pos += 1;
                }
                AppAction::Continue
            }
            KeyCode::Enter => {
                self.confirm_destination();
                AppAction::Continue
            }
            KeyCode::Tab => {
                self.enter_subfolder_creation();
                AppAction::Continue
            }
            KeyCode::Esc => {
                self.cancel_to_browsing();
                AppAction::Continue
            }
            KeyCode::Char(' ') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toggle_quick_look();
                AppAction::Continue
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_query.clear();
                self.search_input_pos = 0;
                self.update_search_results();
                AppAction::Continue
            }
            KeyCode::Char(c) => {
                let byte_pos = self
                    .search_query
                    .char_indices()
                    .nth(self.search_input_pos)
                    .map(|(b, _)| b)
                    .unwrap_or(self.search_query.len());
                self.search_query.insert(byte_pos, c);
                self.search_input_pos += 1;
                self.update_search_results();
                AppAction::Continue
            }
            KeyCode::Backspace => {
                if self.search_input_pos > 0 {
                    let byte_pos = self
                        .search_query
                        .char_indices()
                        .nth(self.search_input_pos - 1)
                        .map(|(b, _)| b)
                        .unwrap_or(self.search_query.len());
                    self.search_query.remove(byte_pos);
                    self.search_input_pos -= 1;
                    self.update_search_results();
                }
                AppAction::Continue
            }
            _ => AppAction::Continue,
        }
    }

    /// Handles key events in SubfolderCreation mode.
    fn handle_subfolder_creation(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Left => {
                if self.search_input_pos > 0 {
                    self.search_input_pos -= 1;
                }
                AppAction::Continue
            }
            KeyCode::Right => {
                let char_count = self.search_query.chars().count();
                if self.search_input_pos < char_count {
                    self.search_input_pos += 1;
                }
                AppAction::Continue
            }
            KeyCode::Enter => {
                self.create_subfolder();
                AppAction::Continue
            }
            KeyCode::Esc => {
                self.cancel_subfolder_creation();
                AppAction::Continue
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.search_query.clear();
                self.search_input_pos = 0;
                AppAction::Continue
            }
            KeyCode::Char(c) => {
                let byte_pos = self
                    .search_query
                    .char_indices()
                    .nth(self.search_input_pos)
                    .map(|(b, _)| b)
                    .unwrap_or(self.search_query.len());
                self.search_query.insert(byte_pos, c);
                self.search_input_pos += 1;
                AppAction::Continue
            }
            KeyCode::Backspace => {
                if self.search_input_pos > 0 {
                    let byte_pos = self
                        .search_query
                        .char_indices()
                        .nth(self.search_input_pos - 1)
                        .map(|(b, _)| b)
                        .unwrap_or(self.search_query.len());
                    self.search_query.remove(byte_pos);
                    self.search_input_pos -= 1;
                }
                AppAction::Continue
            }
            _ => AppAction::Continue,
        }
    }

    /// Handles key events in Naming mode.
    fn handle_naming(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Left => {
                if self.name_input_pos > 0 {
                    self.name_input_pos -= 1;
                }
                AppAction::Continue
            }
            KeyCode::Right => {
                let char_count = self.name_input.chars().count();
                if self.name_input_pos < char_count {
                    self.name_input_pos += 1;
                }
                AppAction::Continue
            }
            KeyCode::Enter => self.confirm_move(),
            KeyCode::Esc => {
                if self.name_input.is_empty() {
                    self.cancel_naming();
                } else {
                    self.name_input.clear();
                    self.name_input_pos = 0;
                }
                AppAction::Continue
            }
            KeyCode::Char(' ') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toggle_quick_look();
                AppAction::Continue
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.name_input.clear();
                self.name_input_pos = 0;
                AppAction::Continue
            }
            KeyCode::Char(c) => {
                let byte_pos = self
                    .name_input
                    .char_indices()
                    .nth(self.name_input_pos)
                    .map(|(b, _)| b)
                    .unwrap_or(self.name_input.len());
                self.name_input.insert(byte_pos, c);
                self.name_input_pos += 1;
                AppAction::Continue
            }
            KeyCode::Backspace => {
                if self.name_input_pos > 0 {
                    let byte_pos = self
                        .name_input
                        .char_indices()
                        .nth(self.name_input_pos - 1)
                        .map(|(b, _)| b)
                        .unwrap_or(self.name_input.len());
                    self.name_input.remove(byte_pos);
                    self.name_input_pos -= 1;
                }
                AppAction::Continue
            }
            _ => AppAction::Continue,
        }
    }

    /// Moves the file list cursor up, closing Quick Look if the file changes.
    fn move_cursor_up(&mut self) {
        if self.cursor > 0 {
            self.close_quick_look_on_cursor_change();
            self.cursor -= 1;
        }
    }

    /// Moves the file list cursor down, closing Quick Look if the file changes.
    fn move_cursor_down(&mut self) {
        if self.cursor + 1 < self.files.len() {
            self.close_quick_look_on_cursor_change();
            self.cursor += 1;
        }
    }

    /// Closes Quick Look when the cursor moves to a different file.
    fn close_quick_look_on_cursor_change(&mut self) {
        if let Some(file) = self.files.get(self.cursor) {
            let effective_path = self.effective_path(&file.path);
            if self.quick_look.is_open_for(&effective_path) {
                self.quick_look.close();
            }
        }
    }

    /// Toggles Quick Look for the currently highlighted file.
    fn toggle_quick_look(&mut self) {
        if let Some(file) = self.files.get(self.cursor) {
            let effective_path = self.effective_path(&file.path);
            self.quick_look.toggle(&effective_path);
        }
    }

    /// Opens the most recently moved-to destination directory in Finder.
    fn open_last_dest_in_finder(&mut self) {
        if let Some(dest) = self.moved.values().last() {
            if let Some(parent) = dest.parent() {
                let _ = std::process::Command::new("open").arg(parent).spawn();
            }
        }
    }

    /// Opens the highlighted file with its default application.
    ///
    /// Uses the effective path (post-move destination if the file was moved this
    /// session). The command is spawned in the background; errors are ignored.
    fn open_highlighted_file(&mut self) {
        if let Some(file) = self.files.get(self.cursor) {
            let path = self.effective_path(&file.path);
            let _ = std::process::Command::new("open")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    /// Reveals the highlighted file in Finder.
    ///
    /// Uses the effective path (post-move destination if the file was moved this
    /// session). The command is spawned in the background; errors are ignored.
    fn reveal_highlighted_file_in_finder(&mut self) {
        if let Some(file) = self.files.get(self.cursor) {
            let path = self.effective_path(&file.path);
            let _ = std::process::Command::new("open")
                .arg("-R")
                .arg(&path)
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
        }
    }

    /// Transitions from Browsing to Searching mode.
    fn enter_searching(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.state = AppState::Searching;
        self.search_query.clear();
        self.search_input_pos = 0;
        self.search_cursor = 0;
        if self.dest_history.is_empty() {
            self.search_results = self.dest_index.top_level_dirs();
        } else {
            self.search_results = self.dest_history.clone();
        }
    }

    /// Transitions from Searching to Naming mode, recording the selected destination.
    fn confirm_destination(&mut self) {
        let Some(dest) = self.search_results.get(self.search_cursor).cloned() else {
            return;
        };
        self.selected_dest = Some(dest);
        self.state = AppState::Naming;
        self.name_input.clear();
        self.name_input_pos = 0;
    }

    /// Transitions from Searching to SubfolderCreation mode.
    fn enter_subfolder_creation(&mut self) {
        if self.search_results.is_empty() {
            return;
        }
        self.state = AppState::SubfolderCreation;
        // Repurpose search_query for folder name input; save cursor position.
        self.search_query.clear();
        self.search_input_pos = 0;
    }

    /// Cancels Searching and returns to Browsing.
    fn cancel_to_browsing(&mut self) {
        self.state = AppState::Browsing;
        self.search_query.clear();
        self.search_input_pos = 0;
        self.search_results.clear();
        self.search_cursor = 0;
    }

    /// Composes the subfolder path and transitions directly to Naming.
    ///
    /// The folder is not created on disk here; `move_file` calls
    /// `fs::create_dir_all` on the destination, so it will be created
    /// automatically when the move is confirmed.
    fn create_subfolder(&mut self) {
        let folder_name = self.search_query.trim().to_string();
        if folder_name.is_empty() {
            self.state = AppState::Searching;
            self.search_query.clear();
            self.search_input_pos = 0;
            return;
        }

        let Some(parent) = self.search_results.get(self.search_cursor).cloned() else {
            self.state = AppState::Searching;
            self.search_query.clear();
            self.search_input_pos = 0;
            return;
        };

        self.selected_dest = Some(format!("{parent}/{folder_name}"));
        self.search_query.clear();
        self.search_input_pos = 0;
        self.state = AppState::Naming;
        self.name_input.clear();
        self.name_input_pos = 0;
    }

    /// Cancels subfolder creation and returns to Searching.
    fn cancel_subfolder_creation(&mut self) {
        self.state = AppState::Searching;
        self.search_query.clear();
        self.search_input_pos = 0;
        self.update_search_results();
    }

    /// Transitions from Browsing to ConfirmDelete mode for the highlighted file.
    fn enter_confirm_delete(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.state = AppState::ConfirmDelete;
    }

    /// Handles key events in ConfirmDelete mode.
    fn handle_confirm_delete(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Char('y') | KeyCode::Char('Y') => self.confirm_delete(),
            _ => {
                self.state = AppState::Browsing;
                AppAction::Continue
            }
        }
    }

    /// Deletes the highlighted file from disk and from the file list.
    fn confirm_delete(&mut self) -> AppAction {
        let Some(file) = self.files.get(self.cursor) else {
            self.state = AppState::Browsing;
            return AppAction::Continue;
        };

        let path = self.effective_path(&file.path);
        let original_path = file.path.clone();

        match std::fs::remove_file(&path) {
            Ok(()) => {
                self.quick_look.close();
                self.files.remove(self.cursor);
                // Clamp cursor so it remains in bounds.
                if self.cursor > 0 && self.cursor >= self.files.len() {
                    self.cursor = self.files.len().saturating_sub(1);
                }
                self.state = AppState::Browsing;
                AppAction::DeleteFile {
                    path: original_path,
                }
            }
            Err(e) => {
                self.error_message = Some(format!("Delete failed: {e}"));
                self.state = AppState::Browsing;
                AppAction::Continue
            }
        }
    }

    /// Cancels Naming and returns to Searching.
    fn cancel_naming(&mut self) {
        self.state = AppState::Searching;
        self.name_input.clear();
        self.name_input_pos = 0;
        // Restore search state.
        self.search_query.clear();
        self.search_input_pos = 0;
        self.update_search_results();
    }

    /// Attempts to move the file and returns the appropriate action.
    fn confirm_move(&mut self) -> AppAction {
        let Some(file) = self.files.get(self.cursor) else {
            return AppAction::Continue;
        };
        let Some(dest_rel) = self.selected_dest.clone() else {
            return AppAction::Continue;
        };

        let src = self.effective_path(&file.path);
        let dest_dir = self.dest_root.join(&dest_rel);
        let final_name = if self.name_input.trim().is_empty() {
            file.filename.clone()
        } else {
            self.build_final_name(&file.filename)
        };

        match file_ops::move_file(&src, &dest_dir, &final_name) {
            Ok(dest_path) => {
                self.quick_look.close();
                self.moved.insert(file.path.clone(), dest_path.clone());
                self.state = AppState::Browsing;
                self.name_input.clear();
                self.name_input_pos = 0;
                self.search_query.clear();
                self.search_input_pos = 0;

                // Record destination in session history, most-recent first.
                self.dest_history.retain(|entry| *entry != dest_rel);
                self.dest_history.insert(0, dest_rel);
                self.dest_history.truncate(MAX_DEST_HISTORY);

                self.selected_dest = None;

                // Rebuild so any newly created subfolder appears in future searches.
                if let Err(e) = self.dest_index.rebuild(&self.dest_root) {
                    self.error_message = Some(format!("Failed to rebuild index: {e}"));
                }

                AppAction::CommitMove {
                    src: file.path.clone(),
                    dest: dest_path,
                    name: final_name,
                }
            }
            Err(e) => {
                self.error_message = Some(format!("Move failed: {e}"));
                AppAction::Continue
            }
        }
    }

    /// Builds the final filename by combining user input with the original extension.
    fn build_final_name(&self, original: &str) -> String {
        let ext = std::path::Path::new(original)
            .extension()
            .map(|e| format!(".{}", e.to_string_lossy()))
            .unwrap_or_default();
        let stem = self.name_input.trim();
        format!("{stem}{ext}")
    }

    /// Updates fuzzy search results from the current query.
    ///
    /// When the query is empty, shows `dest_history` (most-recent first).
    /// When the query is non-empty, runs a fuzzy search across all folders.
    fn update_search_results(&mut self) {
        if self.search_query.is_empty() {
            self.search_results = if self.dest_history.is_empty() {
                self.dest_index.top_level_dirs()
            } else {
                self.dest_history.clone()
            };
            self.search_cursor = 0;
        } else {
            self.search_results = self.dest_index.query(&self.search_query);
            // Clamp search cursor to valid range.
            if self.search_results.is_empty() {
                self.search_cursor = 0;
            } else if self.search_cursor >= self.search_results.len() {
                self.search_cursor = self.search_results.len() - 1;
            }
        }
    }

    /// Moves the search results cursor up.
    fn move_search_cursor_up(&mut self) {
        if self.search_cursor > 0 {
            self.search_cursor -= 1;
        }
    }

    /// Moves the search results cursor down.
    fn move_search_cursor_down(&mut self) {
        if self.search_cursor + 1 < self.search_results.len() {
            self.search_cursor += 1;
        }
    }

    /// Returns the effective filesystem path for a file, resolving any
    /// previous moves recorded in the session.
    pub fn effective_path(&self, original: &PathBuf) -> PathBuf {
        self.moved
            .get(original)
            .cloned()
            .unwrap_or_else(|| original.clone())
    }

    /// Returns the currently highlighted file, if any.
    pub fn current_file(&self) -> Option<&InboxFile> {
        self.files.get(self.cursor)
    }

    /// Returns the desired inline viewport height for the current state.
    ///
    /// In `Browsing` and `ConfirmDelete` modes the viewport is sized to the
    /// file list. In all other modes (`Searching`, `SubfolderCreation`,
    /// `Naming`) the result list may be arbitrarily long, so the full
    /// `MAX_LIST_HEIGHT` is used.
    pub fn desired_viewport_height(&self) -> u16 {
        match self.state {
            AppState::Browsing | AppState::ConfirmDelete => viewport_height(self.files.len()),
            AppState::Searching | AppState::SubfolderCreation | AppState::Naming => MAX_LIST_HEIGHT,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::path::PathBuf;
    use std::time::SystemTime;

    use crate::config::{Config, DestinationConfig, InboxConfig};

    #[test]
    fn viewport_height_zero_files_returns_minimum() {
        assert_eq!(viewport_height(0), MIN_VIEWPORT_HEIGHT);
    }

    #[test]
    fn viewport_height_few_files_uses_content_size() {
        // 3 files → 3 + 3 = 6, which is above MIN and below MAX.
        assert_eq!(viewport_height(3), 6);
    }

    #[test]
    fn viewport_height_many_files_caps_at_max() {
        assert_eq!(viewport_height(100), MAX_LIST_HEIGHT);
    }

    #[test]
    fn viewport_height_exact_max_boundary() {
        // MAX_LIST_HEIGHT files + 2 would exceed max; should cap.
        assert_eq!(viewport_height(MAX_LIST_HEIGHT as usize), MAX_LIST_HEIGHT);
    }

    #[test]
    fn viewport_height_just_below_min() {
        // 1 file → 1 + 3 = 4, equals MIN_VIEWPORT_HEIGHT → clamped to 4.
        assert_eq!(viewport_height(1), MIN_VIEWPORT_HEIGHT);
    }

    fn make_inbox_file(label: &str, filename: &str, path: &str) -> InboxFile {
        InboxFile {
            label: label.to_string(),
            path: PathBuf::from(path),
            filename: filename.to_string(),
            modified: SystemTime::now(),
        }
    }

    fn make_config(dest_root: &str) -> Config {
        Config {
            inboxes: vec![InboxConfig {
                label: Some("Test".to_string()),
                path: PathBuf::from("/tmp/test-inbox"),
            }],
            destination: DestinationConfig {
                root: PathBuf::from(dest_root),
            },
        }
    }

    fn make_key_event(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::empty(),
        }
    }

    fn make_ctrl_key_event(code: KeyCode) -> KeyEvent {
        KeyEvent {
            code,
            modifiers: KeyModifiers::CONTROL,
            kind: KeyEventKind::Press,
            state: crossterm::event::KeyEventState::empty(),
        }
    }

    fn make_app_with_files(files: Vec<InboxFile>) -> App {
        let dir = tempfile::TempDir::new().unwrap();
        std::fs::create_dir(dir.path().join("alpha")).unwrap();
        std::fs::create_dir(dir.path().join("beta")).unwrap();

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let app = App::new(files, index, &config);
        // Keep the temp dir alive by leaking it (the dir is used
        // only for the DestIndex, which has already consumed it).
        let _ = dir.keep();
        app
    }

    #[test]
    fn initial_state_is_browsing() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let app = make_app_with_files(files);
        assert_eq!(app.state, AppState::Browsing);
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn browsing_enter_transitions_to_searching() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        let action = app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Searching);
    }

    #[test]
    fn browsing_enter_with_no_files_stays_browsing() {
        let mut app = make_app_with_files(vec![]);

        let action = app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Browsing);
    }

    #[test]
    fn browsing_cursor_down_moves() {
        let files = vec![
            make_inbox_file("A", "a.pdf", "/tmp/a.pdf"),
            make_inbox_file("B", "b.pdf", "/tmp/b.pdf"),
        ];
        let mut app = make_app_with_files(files);
        assert_eq!(app.cursor, 0);

        app.handle_event(make_key_event(KeyCode::Down));
        assert_eq!(app.cursor, 1);
    }

    #[test]
    fn browsing_cursor_up_at_zero_stays() {
        let files = vec![make_inbox_file("A", "a.pdf", "/tmp/a.pdf")];
        let mut app = make_app_with_files(files);
        assert_eq!(app.cursor, 0);

        app.handle_event(make_key_event(KeyCode::Up));
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn browsing_cursor_down_at_end_stays() {
        let files = vec![make_inbox_file("A", "a.pdf", "/tmp/a.pdf")];
        let mut app = make_app_with_files(files);
        assert_eq!(app.cursor, 0);

        app.handle_event(make_key_event(KeyCode::Down));
        assert_eq!(app.cursor, 0);
    }

    #[test]
    fn searching_esc_returns_to_browsing() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;

        let action = app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Browsing);
    }

    #[test]
    fn searching_typing_updates_query() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;

        app.handle_event(make_key_event(KeyCode::Char('a')));
        assert_eq!(app.search_query, "a");

        app.handle_event(make_key_event(KeyCode::Char('l')));
        assert_eq!(app.search_query, "al");
    }

    #[test]
    fn searching_backspace_removes_char() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        app.search_query = "abc".to_string();
        app.search_input_pos = 3;

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.search_query, "ab");
        assert_eq!(app.search_input_pos, 2);
    }

    #[test]
    fn searching_enter_transitions_to_naming() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        // Ensure there are search results by seeding history.
        app.search_results = app.dest_index.query("alpha");
        assert!(!app.search_results.is_empty());

        let action = app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Naming);
        assert!(app.selected_dest.is_some());
    }

    #[test]
    fn searching_tab_transitions_to_subfolder_creation() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        app.search_results = app.dest_index.query("");

        let action = app.handle_event(make_key_event(KeyCode::Tab));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::SubfolderCreation);
    }

    #[test]
    fn subfolder_creation_esc_returns_to_searching() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::SubfolderCreation;

        let action = app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Searching);
    }

    #[test]
    fn naming_esc_with_empty_input_returns_to_searching() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;

        let action = app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Searching);
    }

    #[test]
    fn naming_esc_with_nonempty_input_clears_and_stays_in_naming() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "some text".to_string();

        let action = app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Naming);
        assert_eq!(app.name_input, "");
    }

    #[test]
    fn naming_esc_twice_clears_then_returns_to_searching() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "some text".to_string();

        app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(app.state, AppState::Naming);
        assert_eq!(app.name_input, "");

        app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(app.state, AppState::Searching);
    }

    #[test]
    fn naming_typing_updates_name_input() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;

        app.handle_event(make_key_event(KeyCode::Char('n')));
        app.handle_event(make_key_event(KeyCode::Char('e')));
        app.handle_event(make_key_event(KeyCode::Char('w')));
        assert_eq!(app.name_input, "new");
    }

    #[test]
    fn double_ctrl_c_quits() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        let action = app.handle_event(make_ctrl_key_event(KeyCode::Char('c')));
        assert_eq!(action, AppAction::Continue);
        assert!(app.error_message.is_some());

        let action = app.handle_event(make_ctrl_key_event(KeyCode::Char('c')));
        assert_eq!(action, AppAction::Quit);
    }

    #[test]
    fn single_ctrl_c_then_other_key_resets() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        app.handle_event(make_ctrl_key_event(KeyCode::Char('c')));
        assert!(app.last_ctrl_c.is_some());

        // Another key resets the timer.
        app.handle_event(make_key_event(KeyCode::Down));
        assert!(app.last_ctrl_c.is_none());
    }

    #[test]
    fn release_events_are_ignored() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        let release_event = KeyEvent {
            code: KeyCode::Enter,
            modifiers: KeyModifiers::empty(),
            kind: KeyEventKind::Release,
            state: crossterm::event::KeyEventState::empty(),
        };
        let action = app.handle_event(release_event);
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Browsing);
    }

    #[test]
    fn confirm_move_with_empty_name_keeps_original() {
        let dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();

        // Set up a real file to move.
        let src_path = src_dir.path().join("doc.pdf");
        std::fs::write(&src_path, b"content").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();

        let files = vec![InboxFile {
            label: "Inbox".to_string(),
            path: src_path.clone(),
            filename: "doc.pdf".to_string(),
            modified: SystemTime::now(),
        }];

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let mut app = App::new(files, index, &config);

        app.state = AppState::Naming;
        app.selected_dest = Some("dest".to_string());
        app.name_input.clear(); // empty means keep original

        let action = app.handle_event(make_key_event(KeyCode::Enter));
        match action {
            AppAction::CommitMove { name, .. } => {
                assert_eq!(name, "doc.pdf");
            }
            other => panic!("expected CommitMove, got {other:?}"),
        }
        assert_eq!(app.state, AppState::Browsing);
    }

    #[test]
    fn confirm_move_with_custom_name_preserves_extension() {
        let dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();

        let src_path = src_dir.path().join("doc.pdf");
        std::fs::write(&src_path, b"content").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();

        let files = vec![InboxFile {
            label: "Inbox".to_string(),
            path: src_path.clone(),
            filename: "doc.pdf".to_string(),
            modified: SystemTime::now(),
        }];

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let mut app = App::new(files, index, &config);

        app.state = AppState::Naming;
        app.selected_dest = Some("dest".to_string());
        app.name_input = "invoice_2024".to_string();

        let action = app.handle_event(make_key_event(KeyCode::Enter));
        match action {
            AppAction::CommitMove { name, .. } => {
                assert_eq!(name, "invoice_2024.pdf");
            }
            other => panic!("expected CommitMove, got {other:?}"),
        }
    }

    #[test]
    fn moved_files_are_tracked() {
        let dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();

        let src_path = src_dir.path().join("doc.pdf");
        std::fs::write(&src_path, b"content").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();

        let files = vec![InboxFile {
            label: "Inbox".to_string(),
            path: src_path.clone(),
            filename: "doc.pdf".to_string(),
            modified: SystemTime::now(),
        }];

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let mut app = App::new(files, index, &config);

        app.state = AppState::Naming;
        app.selected_dest = Some("dest".to_string());

        app.handle_event(make_key_event(KeyCode::Enter));
        assert!(app.moved.contains_key(&src_path));
    }

    #[test]
    fn effective_path_resolves_moved_files() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        let original = PathBuf::from("/tmp/doc.pdf");
        let new_dest = PathBuf::from("/archive/Finance/doc.pdf");
        app.moved.insert(original.clone(), new_dest.clone());

        assert_eq!(app.effective_path(&original), new_dest);
    }

    #[test]
    fn effective_path_returns_original_if_not_moved() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let app = make_app_with_files(files);

        let original = PathBuf::from("/tmp/doc.pdf");
        assert_eq!(app.effective_path(&original), original);
    }

    #[test]
    fn search_cursor_navigation() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        // Populate results directly from the index (bypasses history logic).
        app.search_results = app.dest_index.query("");

        // Should have at least 2 results (alpha, beta from make_app_with_files).
        assert!(app.search_results.len() >= 2);
        assert_eq!(app.search_cursor, 0);

        app.handle_event(make_key_event(KeyCode::Down));
        assert_eq!(app.search_cursor, 1);

        app.handle_event(make_key_event(KeyCode::Up));
        assert_eq!(app.search_cursor, 0);

        // Can't go above 0.
        app.handle_event(make_key_event(KeyCode::Up));
        assert_eq!(app.search_cursor, 0);
    }

    #[test]
    fn build_final_name_with_extension() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let app = make_app_with_files(files);

        // Temporarily set name_input via a mutable reference workaround.
        let mut app = app;
        app.name_input = "new_name".to_string();
        assert_eq!(app.build_final_name("original.pdf"), "new_name.pdf");
    }

    #[test]
    fn build_final_name_without_extension() {
        let files = vec![make_inbox_file("Inbox", "readme", "/tmp/readme")];
        let mut app = make_app_with_files(files);
        app.name_input = "new_name".to_string();
        assert_eq!(app.build_final_name("readme"), "new_name");
    }

    #[test]
    fn error_message_cleared_on_next_key() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.error_message = Some("some error".to_string());

        app.handle_event(make_key_event(KeyCode::Down));
        assert!(app.error_message.is_none());
    }

    #[test]
    fn subfolder_creation_enter_with_empty_name_returns_to_searching() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::SubfolderCreation;
        app.search_results = app.dest_index.query("");
        app.search_query.clear();

        app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(app.state, AppState::Searching);
    }

    #[test]
    fn subfolder_creation_enter_with_name_transitions_to_naming_without_creating_dir() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::SubfolderCreation;
        app.search_results = app.dest_index.query("");
        app.search_cursor = 0;
        // Capture the parent that cursor 0 points to before the event fires.
        let parent = app.search_results[0].clone();
        app.search_query = "sub".to_string();

        let action = app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(action, AppAction::Continue);
        // Must transition to Naming, not back to Searching.
        assert_eq!(app.state, AppState::Naming);
        // selected_dest must be the composed path.
        let expected = format!("{parent}/sub");
        assert_eq!(app.selected_dest, Some(expected.clone()));
        // search_query must be cleared.
        assert_eq!(app.search_query, "");
        // The subfolder must NOT exist on disk yet.
        assert!(!app.dest_root.join(&expected).exists());
    }

    #[test]
    fn subfolder_creation_enter_rebuilds_dest_index_only_after_move() {
        let dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();

        std::fs::create_dir(dir.path().join("alpha")).unwrap();

        let src_path = src_dir.path().join("doc.pdf");
        std::fs::write(&src_path, b"content").unwrap();

        let files = vec![InboxFile {
            label: "Inbox".to_string(),
            path: src_path.clone(),
            filename: "doc.pdf".to_string(),
            modified: SystemTime::now(),
        }];

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let mut app = App::new(files, index, &config);

        // Enter SubfolderCreation with "alpha" as selected result.
        app.state = AppState::SubfolderCreation;
        app.search_results = vec!["alpha".to_string()];
        app.search_cursor = 0;
        app.search_query = "newsub".to_string();

        app.handle_event(make_key_event(KeyCode::Enter));
        // Now in Naming state.
        assert_eq!(app.state, AppState::Naming);
        assert_eq!(app.selected_dest, Some("alpha/newsub".to_string()));

        // Confirm the move — this should create the directory and move the file.
        let action = app.handle_event(make_key_event(KeyCode::Enter));
        match action {
            AppAction::CommitMove { dest, .. } => {
                assert!(dest.exists(), "moved file should exist at dest");
                assert!(
                    dir.path().join("alpha/newsub").is_dir(),
                    "newsub dir should be created"
                );
            }
            other => panic!("expected CommitMove, got {other:?}"),
        }
        // After confirm_move, dest_index should include the new subfolder.
        let results = app.dest_index.query("newsub");
        assert!(
            results.iter().any(|r| r.contains("newsub")),
            "index should include newsub after move"
        );
    }

    #[test]
    fn desired_viewport_height_browsing_uses_file_count() {
        // 3 files → viewport_height(3) = 6.
        let files = vec![
            make_inbox_file("A", "a.pdf", "/tmp/a.pdf"),
            make_inbox_file("B", "b.pdf", "/tmp/b.pdf"),
            make_inbox_file("C", "c.pdf", "/tmp/c.pdf"),
        ];
        let app = make_app_with_files(files);
        assert_eq!(app.state, AppState::Browsing);
        assert_eq!(app.desired_viewport_height(), viewport_height(3));
    }

    #[test]
    fn desired_viewport_height_searching_uses_max() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        assert_eq!(app.desired_viewport_height(), MAX_LIST_HEIGHT);
    }

    #[test]
    fn desired_viewport_height_subfolder_creation_uses_max() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::SubfolderCreation;
        assert_eq!(app.desired_viewport_height(), MAX_LIST_HEIGHT);
    }

    #[test]
    fn desired_viewport_height_naming_uses_max() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        assert_eq!(app.desired_viewport_height(), MAX_LIST_HEIGHT);
    }

    #[test]
    fn commit_move_prepends_to_dest_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();

        let src_path = src_dir.path().join("doc.pdf");
        std::fs::write(&src_path, b"content").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();

        let files = vec![InboxFile {
            label: "Inbox".to_string(),
            path: src_path.clone(),
            filename: "doc.pdf".to_string(),
            modified: SystemTime::now(),
        }];

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let mut app = App::new(files, index, &config);

        app.state = AppState::Naming;
        app.selected_dest = Some("dest".to_string());

        app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(app.dest_history, vec!["dest".to_string()]);
    }

    #[test]
    fn commit_move_deduplicates_dest_history() {
        let dir = tempfile::TempDir::new().unwrap();
        let src_dir = tempfile::TempDir::new().unwrap();

        let src_path1 = src_dir.path().join("a.pdf");
        let src_path2 = src_dir.path().join("b.pdf");
        std::fs::write(&src_path1, b"aaa").unwrap();
        std::fs::write(&src_path2, b"bbb").unwrap();
        std::fs::create_dir(dir.path().join("dest")).unwrap();

        let files = vec![
            InboxFile {
                label: "Inbox".to_string(),
                path: src_path1.clone(),
                filename: "a.pdf".to_string(),
                modified: SystemTime::now(),
            },
            InboxFile {
                label: "Inbox".to_string(),
                path: src_path2.clone(),
                filename: "b.pdf".to_string(),
                modified: SystemTime::now(),
            },
        ];

        let config = make_config(dir.path().to_str().unwrap());
        let index = DestIndex::new(dir.path()).unwrap();
        let mut app = App::new(files, index, &config);

        // Move first file to "dest".
        app.state = AppState::Naming;
        app.selected_dest = Some("dest".to_string());
        app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(app.dest_history.len(), 1);

        // Simulate moving second file to "dest" again.
        app.state = AppState::Naming;
        app.selected_dest = Some("dest".to_string());
        // Need a second file at cursor=0 still there; files list still has entry at index 0.
        // The first file was moved but still in the list; just re-use cursor at the second entry.
        app.cursor = 1;
        app.handle_event(make_key_event(KeyCode::Enter));

        // "dest" must appear only once, at the front.
        assert_eq!(app.dest_history, vec!["dest".to_string()]);
    }

    #[test]
    fn enter_searching_with_history_populates_results() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.dest_history = vec!["alpha".to_string(), "beta".to_string()];

        app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(app.state, AppState::Searching);
        assert_eq!(app.search_results, vec!["alpha", "beta"]);
    }

    #[test]
    fn enter_searching_without_history_shows_top_level_dirs() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(app.state, AppState::Searching);
        // make_app_with_files creates "alpha" and "beta" at the root level.
        assert_eq!(app.search_results, vec!["alpha", "beta"]);
    }

    #[test]
    fn update_search_results_empty_query_empty_history_uses_top_level_dirs() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        // dest_history is empty by default; make_app_with_files creates "alpha" and "beta".

        app.update_search_results();
        assert_eq!(app.search_results, vec!["alpha", "beta"]);
        assert_eq!(app.search_cursor, 0);
    }

    #[test]
    fn update_search_results_empty_query_uses_history() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.dest_history = vec!["alpha".to_string()];

        app.update_search_results();
        assert_eq!(app.search_results, vec!["alpha"]);
        assert_eq!(app.search_cursor, 0);
    }

    #[test]
    fn update_search_results_nonempty_query_uses_fuzzy_index() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.dest_history = vec!["alpha".to_string()];
        app.search_query = "beta".to_string();

        app.update_search_results();
        // Fuzzy search for "beta" should find "beta", not history.
        assert!(app.search_results.iter().any(|r| r.contains("beta")));
        // "alpha" should not appear unless it fuzzy-matches "beta".
        assert!(!app.search_results.iter().all(|r| r == "alpha"));
    }

    #[test]
    fn browsing_ctrl_o_returns_continue() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        let action = app.handle_event(make_ctrl_key_event(KeyCode::Char('o')));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Browsing);
    }

    #[test]
    fn browsing_ctrl_r_returns_continue() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        let action = app.handle_event(make_ctrl_key_event(KeyCode::Char('r')));
        assert_eq!(action, AppAction::Continue);
        assert_eq!(app.state, AppState::Browsing);
    }

    #[test]
    fn browsing_ctrl_o_with_no_files_returns_continue() {
        let mut app = make_app_with_files(vec![]);

        let action = app.handle_event(make_ctrl_key_event(KeyCode::Char('o')));
        assert_eq!(action, AppAction::Continue);
    }

    #[test]
    fn browsing_ctrl_r_with_no_files_returns_continue() {
        let mut app = make_app_with_files(vec![]);

        let action = app.handle_event(make_ctrl_key_event(KeyCode::Char('r')));
        assert_eq!(action, AppAction::Continue);
    }

    #[test]
    fn dest_history_capped_at_max() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);

        // Pre-fill history to the limit.
        app.dest_history = (0..MAX_DEST_HISTORY)
            .map(|i| format!("folder{i}"))
            .collect();

        // Directly invoke history update logic as confirm_move would.
        let new_dest = "extra_folder".to_string();
        app.dest_history.retain(|entry| *entry != new_dest);
        app.dest_history.insert(0, new_dest.clone());
        app.dest_history.truncate(MAX_DEST_HISTORY);

        assert_eq!(app.dest_history.len(), MAX_DEST_HISTORY);
        assert_eq!(app.dest_history[0], new_dest);
    }

    #[test]
    fn searching_left_right_moves_cursor() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        app.search_query = "abc".to_string();
        app.search_input_pos = 3;

        app.handle_event(make_key_event(KeyCode::Left));
        assert_eq!(app.search_input_pos, 2);

        app.handle_event(make_key_event(KeyCode::Left));
        assert_eq!(app.search_input_pos, 1);

        app.handle_event(make_key_event(KeyCode::Right));
        assert_eq!(app.search_input_pos, 2);

        // Cannot go past end.
        app.handle_event(make_key_event(KeyCode::Right));
        app.handle_event(make_key_event(KeyCode::Right));
        assert_eq!(app.search_input_pos, 3);

        // Cannot go below zero.
        app.handle_event(make_key_event(KeyCode::Left));
        app.handle_event(make_key_event(KeyCode::Left));
        app.handle_event(make_key_event(KeyCode::Left));
        app.handle_event(make_key_event(KeyCode::Left));
        assert_eq!(app.search_input_pos, 0);
    }

    #[test]
    fn searching_insert_at_cursor_position() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        app.search_query = "ac".to_string();
        app.search_input_pos = 1;

        app.handle_event(make_key_event(KeyCode::Char('b')));
        assert_eq!(app.search_query, "abc");
        assert_eq!(app.search_input_pos, 2);
    }

    #[test]
    fn searching_backspace_at_middle_removes_left_char() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        app.search_query = "abc".to_string();
        app.search_input_pos = 2;

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.search_query, "ac");
        assert_eq!(app.search_input_pos, 1);
    }

    #[test]
    fn searching_backspace_at_zero_does_nothing() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        app.search_query = "abc".to_string();
        app.search_input_pos = 0;

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.search_query, "abc");
        assert_eq!(app.search_input_pos, 0);
    }

    #[test]
    fn naming_left_right_moves_cursor() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "hi".to_string();
        app.name_input_pos = 2;

        app.handle_event(make_key_event(KeyCode::Left));
        assert_eq!(app.name_input_pos, 1);

        app.handle_event(make_key_event(KeyCode::Right));
        assert_eq!(app.name_input_pos, 2);

        // Cannot exceed length.
        app.handle_event(make_key_event(KeyCode::Right));
        assert_eq!(app.name_input_pos, 2);

        // Cannot go below zero.
        app.handle_event(make_key_event(KeyCode::Left));
        app.handle_event(make_key_event(KeyCode::Left));
        app.handle_event(make_key_event(KeyCode::Left));
        assert_eq!(app.name_input_pos, 0);
    }

    #[test]
    fn naming_insert_at_cursor_position() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "ac".to_string();
        app.name_input_pos = 1;

        app.handle_event(make_key_event(KeyCode::Char('b')));
        assert_eq!(app.name_input, "abc");
        assert_eq!(app.name_input_pos, 2);
    }

    #[test]
    fn naming_backspace_at_middle_removes_left_char() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "abc".to_string();
        app.name_input_pos = 2;

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.name_input, "ac");
        assert_eq!(app.name_input_pos, 1);
    }

    #[test]
    fn naming_backspace_at_zero_does_nothing() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "abc".to_string();
        app.name_input_pos = 0;

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.name_input, "abc");
        assert_eq!(app.name_input_pos, 0);
    }

    #[test]
    fn naming_esc_clears_input_and_resets_cursor() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "hello".to_string();
        app.name_input_pos = 3;

        app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(app.name_input, "");
        assert_eq!(app.name_input_pos, 0);
        assert_eq!(app.state, AppState::Naming);
    }

    #[test]
    fn subfolder_creation_insert_and_cursor() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::SubfolderCreation;
        app.search_query = "ac".to_string();
        app.search_input_pos = 1;

        app.handle_event(make_key_event(KeyCode::Char('b')));
        assert_eq!(app.search_query, "abc");
        assert_eq!(app.search_input_pos, 2);

        app.handle_event(make_key_event(KeyCode::Left));
        assert_eq!(app.search_input_pos, 1);

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.search_query, "bc");
        assert_eq!(app.search_input_pos, 0);
    }
}
