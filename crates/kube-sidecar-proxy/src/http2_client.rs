use std::{collections::VecDeque, future::Future, io, sync::Arc};

use bytes::Bytes;
use futures::future::BoxFuture;
use h2::client;
use tokio::{
    runtime::Handle,
    sync::{mpsc, oneshot},
};
use tracing::{debug, warn};

type ConnectFn = Arc<dyn Fn() -> BoxFuture<'static, io::Result<ConnectResult>> + Send + Sync>;

pub struct ConnectResult {
    pub sender: client::SendRequest<Bytes>,
    pub driver: BoxFuture<'static, Result<(), h2::Error>>,
}

impl ConnectResult {
    pub fn new(
        sender: client::SendRequest<Bytes>,
        driver: BoxFuture<'static, Result<(), h2::Error>>,
    ) -> Self {
        Self { sender, driver }
    }
}

pub struct Http2ClientActor {
    tx: mpsc::Sender<ActorMessage>,
}

#[derive(Debug)]
pub struct Http2ClientLease {
    sender: client::SendRequest<Bytes>,
    tx: mpsc::Sender<ActorMessage>,
    broken: bool,
}

enum ActorMessage {
    Acquire {
        responder: oneshot::Sender<io::Result<Http2ClientLease>>,
    },
    Return {
        broken: bool,
    },
    Connected {
        result: ConnectResult,
    },
    ConnectFailed {
        error: io::Error,
    },
    DriverFinished {
        result: Result<(), h2::Error>,
    },
    Shutdown,
}

struct ActorState {
    label: String,
    connect: ConnectFn,
    sender: Option<client::SendRequest<Bytes>>,
    waiters: VecDeque<oneshot::Sender<io::Result<Http2ClientLease>>>,
    connecting: bool,
}

impl Http2ClientActor {
    pub fn spawn(label: impl Into<String>, connect: ConnectFn) -> Arc<Self> {
        let (tx, rx) = mpsc::channel(32);
        let actor = Arc::new(Self { tx: tx.clone() });

        let state = ActorState {
            label: label.into(),
            connect,
            sender: None,
            waiters: VecDeque::new(),
            connecting: false,
        };

        tokio::spawn(run_actor(state, tx.clone(), rx));

        actor
    }

    pub fn connector<F, Fut>(f: F) -> ConnectFn
    where
        F: Fn() -> Fut + Send + Sync + 'static,
        Fut: Future<Output = io::Result<ConnectResult>> + Send + 'static,
    {
        Arc::new(move || {
            let fut = f();
            Box::pin(fut)
        })
    }

    pub async fn acquire(&self) -> io::Result<Http2ClientLease> {
        let (tx, rx) = oneshot::channel();
        self.tx
            .send(ActorMessage::Acquire { responder: tx })
            .await
            .map_err(|_| {
                io::Error::new(io::ErrorKind::BrokenPipe, "HTTP/2 client actor stopped")
            })?;

        rx.await
            .map_err(|_| io::Error::new(io::ErrorKind::BrokenPipe, "HTTP/2 client actor dropped"))?
    }

    pub async fn shutdown(&self) {
        let _ = self.tx.send(ActorMessage::Shutdown).await;
    }
}

impl Drop for Http2ClientActor {
    fn drop(&mut self) {
        match self.tx.try_send(ActorMessage::Shutdown) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(message)) => {
                if let Ok(handle) = Handle::try_current() {
                    let tx = self.tx.clone();
                    handle.spawn(async move {
                        let _ = tx.send(message).await;
                    });
                }
            }
            Err(mpsc::error::TrySendError::Closed(_)) => {}
        }
    }
}

impl Http2ClientLease {
    pub fn sender_mut(&mut self) -> &mut client::SendRequest<Bytes> {
        &mut self.sender
    }

    pub fn mark_broken(&mut self) {
        self.broken = true;
    }
}

impl Drop for Http2ClientLease {
    fn drop(&mut self) {
        let broken = self.broken;
        if let Err(err) = self.tx.try_send(ActorMessage::Return { broken }) {
            let tx = self.tx.clone();
            tokio::spawn(async move {
                if let Err(send_err) = tx.send(err.into_inner()).await {
                    debug!(
                        "HTTP/2 client actor dropped lease return message: {}",
                        send_err
                    );
                }
            });
        }
    }
}

impl From<mpsc::error::TrySendError<ActorMessage>> for ActorMessage {
    fn from(err: mpsc::error::TrySendError<ActorMessage>) -> Self {
        match err {
            mpsc::error::TrySendError::Closed(message)
            | mpsc::error::TrySendError::Full(message) => message,
        }
    }
}

async fn run_actor(
    mut state: ActorState,
    tx: mpsc::Sender<ActorMessage>,
    mut rx: mpsc::Receiver<ActorMessage>,
) {
    while let Some(message) = rx.recv().await {
        match message {
            ActorMessage::Acquire { responder } => {
                if let Some(sender) = state.sender.as_ref() {
                    deliver_lease(sender.clone(), &tx, responder);
                } else {
                    state.waiters.push_back(responder);
                    ensure_connect(&mut state, &tx);
                }
            }
            ActorMessage::Return { broken } => {
                if broken {
                    debug!(
                        "HTTP/2 client {} reported broken lease; reconnecting",
                        state.label
                    );
                    state.sender = None;
                    if !state.waiters.is_empty() {
                        ensure_connect(&mut state, &tx);
                    }
                }
            }
            ActorMessage::Connected { result } => {
                state.connecting = false;
                let ConnectResult { sender, driver } = result;
                spawn_driver(state.label.clone(), driver, tx.clone());
                state.sender = Some(sender);

                if let Some(base) = state.sender.as_ref() {
                    while let Some(responder) = state.waiters.pop_front() {
                        deliver_lease(base.clone(), &tx, responder);
                    }
                }
            }
            ActorMessage::ConnectFailed { error } => {
                state.connecting = false;
                let kind = error.kind();
                let message = error.to_string();
                warn!(
                    "HTTP/2 client {} failed to connect: {}",
                    state.label, message
                );

                while let Some(responder) = state.waiters.pop_front() {
                    let _ = responder.send(Err(io::Error::new(kind, message.clone())));
                }
            }
            ActorMessage::DriverFinished { result } => {
                state.sender = None;
                match result {
                    Ok(()) => {
                        debug!("HTTP/2 client {} connection closed cleanly", state.label);
                    }
                    Err(err) => {
                        warn!(
                            "HTTP/2 client {} connection ended with error: {}",
                            state.label, err
                        );
                    }
                }

                if !state.waiters.is_empty() {
                    ensure_connect(&mut state, &tx);
                }
            }
            ActorMessage::Shutdown => {
                break;
            }
        }
    }

    while let Some(responder) = state.waiters.pop_front() {
        let _ = responder.send(Err(io::Error::new(
            io::ErrorKind::BrokenPipe,
            "HTTP/2 client actor shutdown",
        )));
    }
}

fn ensure_connect(state: &mut ActorState, tx: &mpsc::Sender<ActorMessage>) {
    if !state.connecting {
        state.connecting = true;
        spawn_connect(state.label.clone(), Arc::clone(&state.connect), tx.clone());
    }
}

fn deliver_lease(
    sender: client::SendRequest<Bytes>,
    tx: &mpsc::Sender<ActorMessage>,
    responder: oneshot::Sender<io::Result<Http2ClientLease>>,
) {
    let lease = Http2ClientLease {
        sender,
        tx: tx.clone(),
        broken: false,
    };

    if responder.send(Ok(lease)).is_err() {
        let _ = tx.try_send(ActorMessage::Return { broken: false });
    }
}

fn spawn_connect(label: String, connect: ConnectFn, tx: mpsc::Sender<ActorMessage>) {
    tokio::spawn(async move {
        let message = match connect().await {
            Ok(result) => ActorMessage::Connected { result },
            Err(error) => ActorMessage::ConnectFailed { error },
        };

        if tx.send(message).await.is_err() {
            debug!(
                "HTTP/2 client {} connect result dropped because actor stopped",
                label
            );
        }
    });
}

fn spawn_driver(
    label: String,
    driver: BoxFuture<'static, Result<(), h2::Error>>,
    tx: mpsc::Sender<ActorMessage>,
) {
    tokio::spawn(async move {
        let result = driver.await;
        if tx
            .send(ActorMessage::DriverFinished { result })
            .await
            .is_err()
        {
            debug!(
                "HTTP/2 client {} driver finished after actor stopped",
                label
            );
        }
    });
}
