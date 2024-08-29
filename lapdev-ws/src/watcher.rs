use std::path::Path;

use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};

pub struct FileWatcher {
    rx_event: Receiver<Result<Event, notify::Error>>,
    pub watcher: RecommendedWatcher,
}

impl FileWatcher {
    pub fn new(data_folder: &Path) -> Result<Self> {
        let (tx_event, rx_event) = unbounded();

        let mut watcher = notify::recommended_watcher(tx_event)?;
        watcher.watch(&data_folder.join("workspaces"), RecursiveMode::Recursive)?;

        Ok(Self { rx_event, watcher })
    }

    pub fn watch(&self) {
        tracing::info!("lapdev ws file watcher started");
        while let Ok(event) = self.rx_event.recv() {
            if let Ok(event) = event {
                match event.kind {
                    notify::EventKind::Access(_) => {
                        continue;
                    }
                    notify::EventKind::Any => {}
                    notify::EventKind::Create(_) => {}
                    notify::EventKind::Modify(_) => {}
                    notify::EventKind::Remove(_) => {}
                    notify::EventKind::Other => {}
                }
                println!("paths {:?}", event.paths);
            }
        }
        tracing::info!("lapdev ws file watcher stopped");
    }
}
