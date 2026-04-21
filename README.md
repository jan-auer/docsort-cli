# docsort

A keyboard-driven TUI for filing documents from inbox folders into an organised archive, without leaving the terminal.

## Install

```
cargo install --path .
```

## Config

Create `.docsort.toml` in your current directory or home directory. Pass `--config <path>` to use a different location.

```toml
[[inboxes]]
label = "Downloads"          # optional; defaults to folder name
path = "/path/to/inbox"

[destination]
root = "/path/to/archive"
```

Multiple `[[inboxes]]` entries are supported. The `label` field is optional and defaults to the folder name when omitted.

## Usage

Launch `docsort` from any directory where a `.docsort.toml` is present (or from home if one lives there). The TUI opens inline — your scroll history stays intact above it.

**Browse mode** — all inbox files are listed newest-first by modification date. Use `↑`/`↓` to navigate. Press `Space` to open a Quick Look preview of the highlighted file in a floating macOS window; press `Space` again to close it.

**Filing a file** — press `↵` on any file to start filing it. The inline area redraws with a fuzzy destination search. Your most recently used folders appear at the top; type to search all folders under the archive root by full path.

- `↑`/`↓` navigate results; the right column shows existing files at the highlighted destination.
- `↵` confirms the destination and advances to the rename screen.
- `Tab` creates a new subfolder inside the highlighted destination: type the folder name, then `↵`.
- `Esc` cancels back to the file list.

**Rename screen** — type a new name for the file, or leave the field empty to keep the original. The extension is preserved automatically. Press `↵` to move the file.

After a successful move, a summary line (`✓ filename → destination/name`) is printed permanently above the UI and the file is dimmed in the list. Filing continues with the next file.

**Quitting** — press `Ctrl+C` twice within one second. A single press shows a reminder.

## Requirements

macOS — Quick Look preview relies on `qlmanage`, which is part of macOS.
