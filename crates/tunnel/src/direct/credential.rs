use std::{collections::HashMap, sync::Arc, time::Duration};

use chrono::{DateTime, Utc};
use tokio::{
    sync::{oneshot, RwLock},
    task::JoinHandle,
    time::{self, MissedTickBehavior},
};

const CREDENTIAL_CLEANUP_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Default)]
pub struct DirectCredentialStore {
    tokens: RwLock<HashMap<String, DateTime<Utc>>>,
}

impl DirectCredentialStore {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashMap::new()),
        }
    }

    pub async fn replace(&self, token: impl Into<String>, expires_at: DateTime<Utc>) {
        let mut guard = self.tokens.write().await;
        guard.clear();
        guard.insert(token.into(), expires_at);
        cleanup_expired(&mut guard);
    }

    pub async fn insert(&self, token: impl Into<String>, expires_at: DateTime<Utc>) {
        let mut guard = self.tokens.write().await;
        guard.insert(token.into(), expires_at);
        cleanup_expired(&mut guard);
    }

    pub async fn remove(&self, token: &str) -> bool {
        let mut guard = self.tokens.write().await;
        cleanup_expired(&mut guard);
        match guard.remove(token) {
            Some(expiry) if expiry >= Utc::now() => true,
            _ => false,
        }
    }

    pub async fn contains(&self, token: &str) -> bool {
        let guard = self.tokens.read().await;
        guard
            .get(token)
            .map(|expiry| *expiry >= Utc::now())
            .unwrap_or(false)
    }

    pub async fn cleanup_expired_now(&self) {
        let mut guard = self.tokens.write().await;
        cleanup_expired(&mut guard);
    }
}

fn cleanup_expired(tokens: &mut HashMap<String, DateTime<Utc>>) {
    if tokens.is_empty() {
        return;
    }
    let now = Utc::now();
    tokens.retain(|_, expires_at| *expires_at >= now);
}

pub(super) fn start_credential_cleanup(
    credential: Arc<DirectCredentialStore>,
) -> CredentialCleanupHandle {
    let (shutdown_tx, mut shutdown_rx) = oneshot::channel();
    let mut ticker = time::interval(CREDENTIAL_CLEANUP_INTERVAL);
    ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);
    let task = tokio::spawn(async move {
        loop {
            tokio::select! {
                _ = &mut shutdown_rx => {
                    break;
                }
                _ = ticker.tick() => {
                    credential.cleanup_expired_now().await;
                }
            }
        }
    });

    CredentialCleanupHandle {
        shutdown: Some(shutdown_tx),
        task,
    }
}

pub(super) struct CredentialCleanupHandle {
    shutdown: Option<oneshot::Sender<()>>,
    task: JoinHandle<()>,
}

impl CredentialCleanupHandle {
    pub(super) fn stop(mut self) {
        if let Some(tx) = self.shutdown.take() {
            let _ = tx.send(());
        }
        self.task.abort();
    }
}
