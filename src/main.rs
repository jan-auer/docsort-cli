use std::path::Path;

use anyhow::Result;

mod config;
mod dest;
mod file_ops;
mod inbox;
mod quicklook;

fn main() -> Result<()> {
    let Some(cfg_path) = config::find_default_config() else {
        return Ok(());
    };

    let cfg = config::load_config(&cfg_path)?;

    let files = inbox::scan_inboxes(&cfg)?;
    let mut index = dest::DestIndex::new(&cfg.destination.root)?;

    let _results = index.query("");
    index.rebuild(&cfg.destination.root)?;

    if let Some(first) = files.first() {
        let _label = &first.label;
        let dest_dir = cfg.destination.root.join("unsorted");
        let _moved = file_ops::move_file(&first.path, &dest_dir, &first.filename);
    }

    file_ops::create_dir(Path::new("/tmp/docsort2-placeholder"))?;

    let dest_files = dest::DestIndex::list_files(&cfg.destination.root, "")?;
    drop(dest_files);

    let mut ql = quicklook::QuickLook::new();
    if let Some(first) = files.first() {
        ql.toggle(&first.path);
        ql.open(&first.path);
        let _open = ql.is_open_for(&first.path);
        ql.close();
    }

    Ok(())
}
