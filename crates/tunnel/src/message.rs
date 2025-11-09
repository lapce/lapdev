use std::io;

use quinn::{ReadExactError, RecvStream, SendStream};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

use crate::error::TunnelError;

/// Abstraction representing the remote address a tunnel should connect to.
#[derive(Debug, Clone)]
pub struct TunnelTarget {
    pub host: String,
    pub port: u16,
}

impl TunnelTarget {
    pub fn new(host: impl Into<String>, port: u16) -> Self {
        Self {
            host: host.into(),
            port,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Protocol {
    Tcp,
    Udp,
}

impl Protocol {
    pub fn as_u8(self) -> u8 {
        match self {
            Protocol::Tcp => 0,
            Protocol::Udp => 1,
        }
    }
}

impl TryFrom<u8> for Protocol {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            0 => Ok(Protocol::Tcp),
            1 => Ok(Protocol::Udp),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone)]
pub struct StreamOpenRequest {
    pub protocol: Protocol,
    pub target: TunnelTarget,
}

#[derive(Debug, Clone)]
pub struct StreamOpenResponse {
    pub success: bool,
    pub error: Option<String>,
}

const MAX_HOST_LEN: usize = u16::MAX as usize;
const MAX_ERROR_LEN: usize = u16::MAX as usize;

pub async fn write_stream_open_request(
    stream: &mut SendStream,
    protocol: Protocol,
    target: &TunnelTarget,
) -> Result<(), TunnelError> {
    let host_bytes = target.host.as_bytes();
    if host_bytes.len() > MAX_HOST_LEN {
        return Err(TunnelError::Remote("target host too long".into()));
    }

    stream
        .write_u8(protocol.as_u8())
        .await
        .map_err(to_transport_err)?;
    stream
        .write_u16(host_bytes.len() as u16)
        .await
        .map_err(to_transport_err)?;
    stream
        .write_all(host_bytes)
        .await
        .map_err(to_transport_err)?;
    stream
        .write_u16(target.port)
        .await
        .map_err(to_transport_err)?;
    stream.flush().await.map_err(to_transport_err)?;
    Ok(())
}

pub async fn read_stream_open_request(
    stream: &mut RecvStream,
) -> Result<StreamOpenRequest, TunnelError> {
    let protocol = Protocol::try_from(stream.read_u8().await.map_err(to_transport_err)?)
        .map_err(|_| TunnelError::Remote("unsupported protocol".into()))?;
    let host_len = stream.read_u16().await.map_err(to_transport_err)? as usize;
    if host_len > MAX_HOST_LEN {
        return Err(TunnelError::Remote("target host too large".into()));
    }
    let mut host_bytes = vec![0u8; host_len];
    read_exact_into(stream, &mut host_bytes).await?;
    let host = String::from_utf8(host_bytes)
        .map_err(|_| TunnelError::Remote("invalid target host encoding".into()))?;
    let port = stream.read_u16().await.map_err(to_transport_err)?;

    Ok(StreamOpenRequest {
        protocol,
        target: TunnelTarget::new(host, port),
    })
}

pub async fn write_stream_open_response(
    stream: &mut SendStream,
    success: bool,
    error: Option<&str>,
) -> Result<(), TunnelError> {
    stream
        .write_u8(u8::from(success))
        .await
        .map_err(to_transport_err)?;
    if success {
        stream.flush().await.map_err(to_transport_err)?;
        return Ok(());
    }

    let message = error.unwrap_or("remote open failed");
    let reason_bytes = message.as_bytes();
    let len = reason_bytes.len().min(MAX_ERROR_LEN);
    stream
        .write_u16(len as u16)
        .await
        .map_err(to_transport_err)?;
    stream
        .write_all(&reason_bytes[..len])
        .await
        .map_err(to_transport_err)?;
    stream.flush().await.map_err(to_transport_err)?;
    Ok(())
}

pub async fn read_stream_open_response(
    stream: &mut RecvStream,
) -> Result<StreamOpenResponse, TunnelError> {
    let success = stream.read_u8().await.map_err(to_transport_err)? != 0;
    if success {
        return Ok(StreamOpenResponse {
            success: true,
            error: None,
        });
    }

    let len = stream.read_u16().await.map_err(to_transport_err)? as usize;
    if len > MAX_ERROR_LEN {
        return Err(TunnelError::Remote("error message too large".into()));
    }
    let mut buf = vec![0u8; len];
    read_exact_into(stream, &mut buf).await?;
    let message = String::from_utf8(buf).unwrap_or_else(|_| "remote open failed".to_string());
    Ok(StreamOpenResponse {
        success: false,
        error: Some(message),
    })
}

fn to_transport_err(err: impl Into<io::Error>) -> TunnelError {
    TunnelError::Transport(err.into())
}

async fn read_exact_into(stream: &mut RecvStream, buf: &mut [u8]) -> Result<(), TunnelError> {
    stream.read_exact(buf).await.map_err(|err| match err {
        ReadExactError::FinishedEarly(_) => TunnelError::ConnectionClosed,
        ReadExactError::ReadError(inner) => TunnelError::Transport(io::Error::other(inner)),
    })
}
