use std::io;

#[derive(thiserror::Error, Debug)]
pub enum TunnelError {
    #[error("transport error: {0}")]
    Transport(#[from] io::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("remote error: {0}")]
    Remote(String),
}

impl From<TunnelError> for io::Error {
    fn from(value: TunnelError) -> Self {
        match value {
            TunnelError::Transport(err) => err,
            TunnelError::ConnectionClosed => {
                io::Error::new(io::ErrorKind::BrokenPipe, TunnelError::ConnectionClosed)
            }
            TunnelError::Remote(reason) => io::Error::other(TunnelError::Remote(reason)),
        }
    }
}
