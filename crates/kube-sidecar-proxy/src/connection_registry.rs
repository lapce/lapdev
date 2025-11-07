use std::{collections::HashMap, sync::Arc};

use tokio::sync::{watch, RwLock};
use uuid::Uuid;

type BranchKey = Option<Uuid>;

struct BranchChannel {
    sender: watch::Sender<bool>,
    receiver: watch::Receiver<bool>,
}

impl BranchChannel {
    fn new() -> Self {
        let (sender, receiver) = watch::channel(false);
        Self { sender, receiver }
    }
}

/// Tracks active proxy connections per branch (or shared/default bucket) and
/// provides broadcast-style shutdown signals whenever routing changes.
#[derive(Clone, Default)]
pub struct ConnectionRegistry {
    branches: Arc<RwLock<HashMap<BranchKey, BranchChannel>>>,
}

impl ConnectionRegistry {
    /// Subscribe to shutdown notifications for the given branch bucket.
    pub async fn subscribe(&self, branch: BranchKey) -> watch::Receiver<bool> {
        let mut guard = self.branches.write().await;
        guard
            .entry(branch)
            .or_insert_with(BranchChannel::new)
            .receiver
            .clone()
    }

    /// Broadcasts a shutdown signal to all connections registered under the
    /// provided branch key. Future subscriptions for the same branch will
    /// receive a fresh channel, ensuring they are not immediately cancelled.
    pub async fn shutdown_branch(&self, branch: BranchKey) {
        let mut guard = self.branches.write().await;
        if let Some(channel) = guard.remove(&branch) {
            let _ = channel.sender.send(true);
        }
    }

    /// Convenience helper to broadcast shutdowns to several branches.
    pub async fn shutdown_branches<I>(&self, branches: I)
    where
        I: IntoIterator<Item = BranchKey>,
    {
        for branch in branches {
            self.shutdown_branch(branch).await;
        }
    }
}
