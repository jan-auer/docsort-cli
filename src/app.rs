use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Instant;

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
/// Returns `min(MAX_LIST_HEIGHT, max(MIN_VIEWPORT_HEIGHT, files + 2))` where
/// the `+2` accounts for one separator line and one hint bar line.
pub fn viewport_height(file_count: usize) -> u16 {
    let needed = (file_count as u16).saturating_add(2);
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
    /// The user requested to quit the application.
    Quit,
}

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
    /// Timestamp of the last Ctrl+C press, for double-tap quit.
    pub last_ctrl_c: Option<Instant>,
    /// Error message to display in the hint bar.
    pub error_message: Option<String>,
    /// Root path of the destination archive.
    pub dest_root: PathBuf,
    /// The destination path chosen in Searching mode, carried into Naming.
    pub selected_dest: Option<String>,
}

impl App {
    /// Creates a new `App` from scanned inbox files, a destination index, and config.
    pub fn new(files: Vec<InboxFile>, dest_index: DestIndex, config: &Config) -> Self {
        let search_results = dest_index.query("");
        Self {
            files,
            cursor: 0,
            moved: HashMap::new(),
            dest_index,
            quick_look: QuickLook::new(),
            state: AppState::Browsing,
            search_query: String::new(),
            search_results,
            search_cursor: 0,
            name_input: String::new(),
            last_ctrl_c: None,
            error_message: None,
            dest_root: config.destination.root.clone(),
            selected_dest: None,
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
            KeyCode::Char('o') => {
                self.open_last_dest_in_finder();
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
            KeyCode::Char(c) => {
                self.search_query.push(c);
                self.update_search_results();
                AppAction::Continue
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                self.update_search_results();
                AppAction::Continue
            }
            _ => AppAction::Continue,
        }
    }

    /// Handles key events in SubfolderCreation mode.
    fn handle_subfolder_creation(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Enter => {
                self.create_subfolder();
                AppAction::Continue
            }
            KeyCode::Esc => {
                self.cancel_subfolder_creation();
                AppAction::Continue
            }
            KeyCode::Char(c) => {
                self.search_query.push(c);
                AppAction::Continue
            }
            KeyCode::Backspace => {
                self.search_query.pop();
                AppAction::Continue
            }
            _ => AppAction::Continue,
        }
    }

    /// Handles key events in Naming mode.
    fn handle_naming(&mut self, event: KeyEvent) -> AppAction {
        match event.code {
            KeyCode::Enter => self.confirm_move(),
            KeyCode::Esc => {
                self.cancel_naming();
                AppAction::Continue
            }
            KeyCode::Char('u') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.name_input.clear();
                AppAction::Continue
            }
            KeyCode::Char(' ') if event.modifiers.contains(KeyModifiers::CONTROL) => {
                self.toggle_quick_look();
                AppAction::Continue
            }
            KeyCode::Char(c) => {
                self.name_input.push(c);
                AppAction::Continue
            }
            KeyCode::Backspace => {
                self.name_input.pop();
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

    /// Transitions from Browsing to Searching mode.
    fn enter_searching(&mut self) {
        if self.files.is_empty() {
            return;
        }
        self.state = AppState::Searching;
        self.search_query.clear();
        self.search_cursor = 0;
        self.update_search_results();
    }

    /// Transitions from Searching to Naming mode, recording the selected destination.
    fn confirm_destination(&mut self) {
        let Some(dest) = self.search_results.get(self.search_cursor).cloned() else {
            return;
        };
        self.selected_dest = Some(dest);
        self.state = AppState::Naming;
        self.name_input.clear();
    }

    /// Transitions from Searching to SubfolderCreation mode.
    fn enter_subfolder_creation(&mut self) {
        if self.search_results.is_empty() {
            return;
        }
        self.state = AppState::SubfolderCreation;
        // Repurpose search_query for folder name input; save cursor position.
        self.search_query.clear();
    }

    /// Cancels Searching and returns to Browsing.
    fn cancel_to_browsing(&mut self) {
        self.state = AppState::Browsing;
        self.search_query.clear();
        self.search_results.clear();
        self.search_cursor = 0;
    }

    /// Creates the subfolder and transitions back to Searching.
    fn create_subfolder(&mut self) {
        let folder_name = self.search_query.trim().to_string();
        if folder_name.is_empty() {
            self.state = AppState::Searching;
            self.search_query.clear();
            return;
        }

        let Some(parent) = self.search_results.get(self.search_cursor).cloned() else {
            self.state = AppState::Searching;
            self.search_query.clear();
            return;
        };

        let new_dir = self.dest_root.join(&parent).join(&folder_name);
        if let Err(e) = file_ops::create_dir(&new_dir) {
            self.error_message = Some(format!("Failed to create folder: {e}"));
            self.state = AppState::Searching;
            self.search_query.clear();
            return;
        }

        if let Err(e) = self.dest_index.rebuild(&self.dest_root) {
            self.error_message = Some(format!("Failed to rebuild index: {e}"));
        }

        // Point search cursor to the new folder.
        let new_rel = format!("{parent}/{folder_name}");
        self.search_query.clear();
        self.update_search_results();

        // Try to find the new folder in results.
        if let Some(pos) = self.search_results.iter().position(|r| *r == new_rel) {
            self.search_cursor = pos;
        }

        self.state = AppState::Searching;
    }

    /// Cancels subfolder creation and returns to Searching.
    fn cancel_subfolder_creation(&mut self) {
        self.state = AppState::Searching;
        self.search_query.clear();
        self.update_search_results();
    }

    /// Cancels Naming and returns to Searching.
    fn cancel_naming(&mut self) {
        self.state = AppState::Searching;
        self.name_input.clear();
        // Restore search state.
        self.search_query.clear();
        self.update_search_results();
    }

    /// Attempts to move the file and returns the appropriate action.
    fn confirm_move(&mut self) -> AppAction {
        let Some(file) = self.files.get(self.cursor) else {
            return AppAction::Continue;
        };
        let Some(ref dest_rel) = self.selected_dest else {
            return AppAction::Continue;
        };

        let src = self.effective_path(&file.path);
        let dest_dir = self.dest_root.join(dest_rel);
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
                self.search_query.clear();
                self.selected_dest = None;

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
    fn update_search_results(&mut self) {
        self.search_results = self.dest_index.query(&self.search_query);
        // Clamp search cursor to valid range.
        if self.search_results.is_empty() {
            self.search_cursor = 0;
        } else if self.search_cursor >= self.search_results.len() {
            self.search_cursor = self.search_results.len() - 1;
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
        // 3 files → 3 + 2 = 5, which is above MIN and below MAX.
        assert_eq!(viewport_height(3), 5);
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
        // 1 file → 1 + 2 = 3, below MIN_VIEWPORT_HEIGHT (4) → clamped to 4.
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
                label: "Test".to_string(),
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

        app.handle_event(make_key_event(KeyCode::Backspace));
        assert_eq!(app.search_query, "ab");
    }

    #[test]
    fn searching_enter_transitions_to_naming() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Searching;
        // Ensure there are search results.
        app.update_search_results();
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
        app.update_search_results();

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
    fn naming_esc_returns_to_searching() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;

        let action = app.handle_event(make_key_event(KeyCode::Esc));
        assert_eq!(action, AppAction::Continue);
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
    fn naming_ctrl_u_clears_input() {
        let files = vec![make_inbox_file("Inbox", "doc.pdf", "/tmp/doc.pdf")];
        let mut app = make_app_with_files(files);
        app.state = AppState::Naming;
        app.name_input = "some text".to_string();

        app.handle_event(make_ctrl_key_event(KeyCode::Char('u')));
        assert_eq!(app.name_input, "");
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
        app.update_search_results();

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
        app.update_search_results();
        app.search_query.clear();

        app.handle_event(make_key_event(KeyCode::Enter));
        assert_eq!(app.state, AppState::Searching);
    }
}
