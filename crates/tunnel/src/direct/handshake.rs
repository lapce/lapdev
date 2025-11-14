use std::io;

use quinn::{RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::TunnelError;

const HANDSHAKE_MAX_TOKEN_LEN: usize = u16::MAX as usize;

pub(crate) async fn write_token(stream: &mut SendStream, token: &str) -> Result<(), TunnelError> {
    let token_bytes = token.as_bytes();
    if token_bytes.len() > HANDSHAKE_MAX_TOKEN_LEN {
        return Err(TunnelError::Remote("direct credential too long".into()));
    }
    stream
        .write_u16(token_bytes.len() as u16)
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    stream
        .write_all(token_bytes)
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    stream
        .flush()
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    Ok(())
}

pub(crate) async fn read_token(stream: &mut RecvStream) -> Result<String, TunnelError> {
    let len = stream
        .read_u16()
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    let mut buf = vec![0u8; len as usize];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    String::from_utf8(buf).map_err(|_| TunnelError::Remote("invalid credential encoding".into()))
}

pub(crate) async fn read_server_ack(stream: &mut RecvStream) -> Result<(), TunnelError> {
    let ack = stream
        .read_u8()
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
    if ack == 1 {
        Ok(())
    } else {
        Err(TunnelError::Remote(
            "direct server rejected credential".into(),
        ))
    }
}

pub(crate) async fn write_server_ack(stream: &mut SendStream) -> Result<(), TunnelError> {
    stream
        .write_all(&[1u8])
        .await
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))
}
