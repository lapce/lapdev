use std::{
    collections::HashMap,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

use anyhow::Result;
use crossbeam_channel::{unbounded, Receiver};
use notify::{Event, RecommendedWatcher, RecursiveMode, Watcher};
use parking_lot::Mutex;

struct PendingPath {
    workspace: String,
    start: Instant,
    last: Instant,
}

pub struct FileWatcher {
    rx_event: Receiver<Result<Event, notify::Error>>,
    pub watcher: RecommendedWatcher,
    base: PathBuf,
    pending: Arc<Mutex<HashMap<PathBuf, PendingPath>>>,
}

impl FileWatcher {
    pub fn new(data_folder: &Path, backup_host: Option<String>) -> Result<Self> {
        let (tx_event, rx_event) = unbounded();

        let folder = data_folder.join("workspaces");
        let mut watcher = notify::recommended_watcher(tx_event)?;
        watcher.watch(&folder, RecursiveMode::Recursive)?;

        let pending: Arc<Mutex<HashMap<PathBuf, PendingPath>>> =
            Arc::new(Mutex::new(HashMap::new()));

        {
            let pending = pending.clone();
            std::thread::spawn(move || loop {
                let last_run = Instant::now();
                let paths = {
                    let mut paths = Vec::new();
                    let mut pending = pending.lock();
                    for (path, pending) in pending.iter() {
                        if pending.last.elapsed().as_millis() > 1000
                            || pending.start.elapsed().as_millis() > 1000 * 10
                        {
                            paths.push((path.clone(), pending.workspace.clone()));
                        }
                    }
                    for (p, _) in &paths {
                        pending.remove(p);
                    }
                    paths
                };
                if let Some(backup_host) = backup_host.as_ref() {
                    for (p, workspace) in paths {
                        let cmd = format!(
                            r#"rsync -a --delete --filter=':- .gitignore' -e 'ssh -o ControlMaster=auto -o ControlPath="~/.ssh/%L-%r@%h:%p"  -o ControlPersist=10m ' {}/ {backup_host}:~/workspaces/{workspace}/"#,
                            p.to_string_lossy()
                        );
                        tracing::debug!("now run {cmd}");
                        if let Err(e) = std::process::Command::new("bash")
                            .arg("-c")
                            .arg(cmd)
                            .output()
                        {
                            tracing::error!("got error when backing up {p:?}: {e:?}");
                        }
                    }
                }

                let elpased = last_run.elapsed().as_millis();
                if elpased < 1000 {
                    std::thread::sleep(Duration::from_millis(1000 - elpased as u64));
                }
            });
        }

        Ok(Self {
            rx_event,
            watcher,
            base: folder,
            pending,
        })
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
                for path in event.paths {
                    if let Ok(p) = path.strip_prefix(&self.base) {
                        let mut components = p.components();
                        if let Some(user) = components.next() {
                            if let Some(workspace) = components.next() {
                                let workspace_full_path = self.base.join(user).join(workspace);
                                let mut pending = self.pending.lock();
                                let pending =
                                    pending.entry(workspace_full_path).or_insert_with(move || {
                                        PendingPath {
                                            workspace: workspace
                                                .as_os_str()
                                                .to_string_lossy()
                                                .to_string(),
                                            start: Instant::now(),
                                            last: Instant::now(),
                                        }
                                    });
                                pending.last = Instant::now();
                            }
                        }
                    }
                }
            }
        }
        tracing::info!("lapdev ws file watcher stopped");
    }
}
