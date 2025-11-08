use std::io;

#[derive(thiserror::Error, Debug)]
pub enum TunnelError {
    #[error("transport error: {0}")]
    Transport(#[from] io::Error),
    #[error("serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("connection closed")]
    ConnectionClosed,
    #[error("remote error: {0}")]
    Remote(String),
}

impl From<TunnelError> for io::Error {
    fn from(value: TunnelError) -> Self {
        match value {
            TunnelError::Transport(err) => err,
            TunnelError::Serialization(err) => io::Error::new(io::ErrorKind::InvalidData, err),
            TunnelError::ConnectionClosed => {
                io::Error::new(io::ErrorKind::BrokenPipe, TunnelError::ConnectionClosed)
            }
            TunnelError::Remote(reason) => {
                io::Error::new(io::ErrorKind::Other, TunnelError::Remote(reason))
            }
        }
    }
}
