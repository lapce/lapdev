use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};

use lapdev_common::devbox::{
    DirectCandidate, DirectCandidateKind, DirectCandidateSet, DirectChannelConfig, DirectTransport,
};
use quinn::rustls;
use quinn::{self, Connection, Endpoint, RecvStream, SendStream};
use rcgen::generate_simple_self_signed;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::RwLock;
use tracing::{debug, info, warn};

use crate::{client::TunnelClient, error::TunnelError};

const HANDSHAKE_MAX_TOKEN_LEN: usize = u16::MAX as usize;

/// Options that influence how direct transport candidates are collected.
#[derive(Debug, Clone)]
pub struct CandidateOptions {
    /// Preferred listening port to advertise to peers. Defaults to 0 (unknown).
    pub preferred_port: Option<u16>,
    /// Whether to include loopback candidates like 127.0.0.1 for debugging.
    pub include_loopback: bool,
    /// Pre-configured relay candidates supplied by the caller.
    pub relay_candidates: Vec<DirectCandidate>,
}

impl Default for CandidateOptions {
    fn default() -> Self {
        Self {
            preferred_port: None,
            include_loopback: true,
            relay_candidates: Vec::new(),
        }
    }
}

/// Basic telemetry describing what the collector discovered.
#[derive(Debug, Clone, Default)]
pub struct CandidateDiagnostics {
    pub detected_interface: Option<IpAddr>,
    pub relay_candidates: usize,
    pub loopback_included: bool,
}

/// Result produced by the candidate collector.
#[derive(Debug, Clone)]
pub struct CandidateDiscovery {
    pub candidates: DirectCandidateSet,
    pub diagnostics: CandidateDiagnostics,
}

/// Collects direct transport candidates based on local interface probing and supplied options.
pub fn collect_candidates(opts: &CandidateOptions) -> CandidateDiscovery {
    let mut diagnostics = CandidateDiagnostics {
        relay_candidates: opts.relay_candidates.len(),
        loopback_included: opts.include_loopback,
        detected_interface: None,
    };

    let mut priority_counter = 0u32;
    let mut candidates = Vec::new();

    if let Some((ip, direct_candidate)) =
        detect_public_interface(opts.preferred_port, &mut priority_counter)
    {
        diagnostics.detected_interface = Some(ip);
        candidates.push(direct_candidate);
    }

    if opts.include_loopback {
        candidates.push(DirectCandidate {
            host: Ipv4Addr::LOCALHOST.to_string(),
            port: opts.preferred_port.unwrap_or(0),
            transport: DirectTransport::Quic,
            kind: DirectCandidateKind::Public,
            priority: next_priority(&mut priority_counter),
        });
    }

    for relay in &opts.relay_candidates {
        let mut relay_candidate = relay.clone();
        relay_candidate.priority = next_priority(&mut priority_counter);
        candidates.push(relay_candidate);
    }

    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_secs())
        .unwrap_or_default();

    CandidateDiscovery {
        candidates: DirectCandidateSet {
            candidates,
            generation: Some(generation),
        },
        diagnostics,
    }
}

fn detect_public_interface(
    preferred_port: Option<u16>,
    priority_counter: &mut u32,
) -> Option<(IpAddr, DirectCandidate)> {
    const PROBE_ADDR: &str = "8.8.8.8:80";

    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.connect(PROBE_ADDR).ok()?;
    let addr = socket.local_addr().ok()?;

    Some((
        addr.ip(),
        DirectCandidate {
            host: addr.ip().to_string(),
            port: preferred_port.unwrap_or(addr.port()),
            transport: DirectTransport::Quic,
            kind: DirectCandidateKind::Public,
            priority: next_priority(priority_counter),
        },
    ))
}

fn next_priority(counter: &mut u32) -> u32 {
    let value = *counter;
    *counter = counter.saturating_add(1);
    value
}

/// Attempt to establish a direct tunnel using the provided channel config.
pub async fn connect_direct_tunnel(
    config: &DirectChannelConfig,
) -> Result<TunnelClient, TunnelError> {
    let mut endpoint = Endpoint::client("[::]:0".parse().unwrap())
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    endpoint.set_default_client_config(client_config());

    let mut last_err: Option<TunnelError> = None;

    for candidate in config
        .devbox_candidates
        .iter()
        .filter(|candidate| matches!(candidate.transport, DirectTransport::Quic))
    {
        let addr_string = format!("{}:{}", candidate.host, candidate.port);
        let resolved_addrs: Vec<SocketAddr> = match addr_string.to_socket_addrs() {
            Ok(iter) => iter.collect(),
            Err(err) => {
                last_err = Some(TunnelError::Transport(err));
                continue;
            }
        };

        for addr in resolved_addrs {
            debug!(candidate = %addr, "Attempting direct QUIC candidate");
            match endpoint.connect(addr, "lapdev-direct") {
                Ok(connecting) => match connecting.await {
                    Ok(connection) => {
                        match establish_client_stream(connection, &config.credential.token).await {
                            Ok(transport) => {
                                info!(peer = %addr, "Direct QUIC tunnel established");
                                return Ok(TunnelClient::connect(transport));
                            }
                            Err(err) => {
                                last_err = Some(err);
                                continue;
                            }
                        }
                    }
                    Err(err) => {
                        warn!(candidate = %addr, error = %err, "Direct QUIC connect failed");
                        last_err = Some(TunnelError::Transport(std::io::Error::other(err)));
                        continue;
                    }
                },
                Err(err) => {
                    warn!(candidate = %addr, error = %err, "Unable to initiate direct QUIC connect");
                    last_err = Some(TunnelError::Transport(std::io::Error::other(err)));
                    continue;
                }
            }
        }
    }

    if last_err.is_some() {
        warn!("All direct QUIC candidates failed; falling back to WebSocket");
    }

    Err(last_err.unwrap_or_else(|| TunnelError::Remote("no QUIC candidates available".to_string())))
}

async fn establish_client_stream(
    connection: Connection,
    token: &str,
) -> Result<QuicTransport, TunnelError> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    write_token(&mut send, token).await?;
    read_server_ack(&mut recv).await?;
    Ok(QuicTransport::new(send, recv))
}

/// QUIC listener that validates the shared credential before handing over the transport.
pub struct DirectQuicServer {
    endpoint: Endpoint,
    credential: Arc<RwLock<Option<String>>>,
}

impl DirectQuicServer {
    pub async fn bind(bind_addr: SocketAddr) -> Result<Self, TunnelError> {
        let server_config = build_server_config()?;
        let endpoint = Endpoint::server(server_config, bind_addr)
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
        Ok(Self {
            endpoint,
            credential: Arc::new(RwLock::new(None)),
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TunnelError> {
        self.endpoint
            .local_addr()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))
    }

    pub fn credential_handle(&self) -> Arc<RwLock<Option<String>>> {
        Arc::clone(&self.credential)
    }

    pub async fn accept(&self) -> Result<QuicTransport, TunnelError> {
        let incoming = match self.endpoint.accept().await {
            Some(incoming) => incoming,
            None => return Err(TunnelError::ConnectionClosed),
        };

        let connecting = incoming
            .accept()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

        let connection = connecting
            .await
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

        let expected = {
            let guard = self.credential.read().await;
            guard
                .clone()
                .ok_or_else(|| TunnelError::Remote("direct credential not configured".into()))?
        };

        let presented = read_token(&mut recv).await?;
        if presented != expected {
            return Err(TunnelError::Remote("invalid direct credential".into()));
        }

        write_server_ack(&mut send).await?;

        Ok(QuicTransport::new(send, recv))
    }

    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"direct server shutdown");
    }
}

/// Transport wrapper around a QUIC bidirectional stream.
pub struct QuicTransport {
    reader: RecvStream,
    writer: SendStream,
}

impl QuicTransport {
    fn new(send: SendStream, recv: RecvStream) -> Self {
        Self {
            reader: recv,
            writer: send,
        }
    }
}

impl AsyncRead for QuicTransport {
    fn poll_read(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        std::pin::Pin::new(&mut this.reader).poll_read(cx, buf)
    }
}

impl AsyncWrite for QuicTransport {
    fn poll_write(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        let this = self.get_mut();
        AsyncWrite::poll_write(std::pin::Pin::new(&mut this.writer), cx, buf)
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        AsyncWrite::poll_flush(std::pin::Pin::new(&mut this.writer), cx)
    }

    fn poll_shutdown(
        self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        let this = self.get_mut();
        AsyncWrite::poll_shutdown(std::pin::Pin::new(&mut this.writer), cx)
    }
}

fn build_server_config() -> Result<quinn::ServerConfig, TunnelError> {
    let cert = generate_simple_self_signed(["lapdev.devbox".to_string()])
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let cert_der = cert
        .serialize_der()
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let priv_key = cert.serialize_private_key_der();

    let cert_chain = vec![CertificateDer::from(cert_der)];
    let private_key = PrivatePkcs8KeyDer::from(priv_key);

    quinn::ServerConfig::with_single_cert(cert_chain, private_key.into())
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))
}

fn client_config() -> quinn::ClientConfig {
    let roots = Arc::new(rustls::RootCertStore::empty());
    let mut crypto =
        rustls::ClientConfig::with_root_certificates(roots).expect("invalid client config");
    crypto
        .dangerous()
        .set_certificate_verifier(Arc::new(SkipServerVerification));
    quinn::ClientConfig::new(Arc::new(crypto))
}

#[derive(Debug)]
struct SkipServerVerification;

impl ServerCertVerifier for SkipServerVerification {
    fn verify_server_cert(
        &self,
        _end_entity: &CertificateDer<'_>,
        _intermediates: &[CertificateDer<'_>],
        _server_name: &ServerName<'_>,
        _ocsp_response: &[u8],
        _now: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }

    fn verify_tls12_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn verify_tls13_signature(
        &self,
        _message: &[u8],
        _cert: &CertificateDer<'_>,
        _dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Ok(HandshakeSignatureValid::assertion())
    }

    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        vec![
            SignatureScheme::RSA_PKCS1_SHA256,
            SignatureScheme::ECDSA_NISTP256_SHA256,
        ]
    }
}

async fn write_token(stream: &mut SendStream, token: &str) -> Result<(), TunnelError> {
    let token_bytes = token.as_bytes();
    if token_bytes.len() > HANDSHAKE_MAX_TOKEN_LEN {
        return Err(TunnelError::Remote("direct credential too long".into()));
    }
    stream
        .write_u16(token_bytes.len() as u16)
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    stream
        .write_all(token_bytes)
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    stream
        .flush()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    Ok(())
}

async fn read_token(stream: &mut RecvStream) -> Result<String, TunnelError> {
    let len = stream
        .read_u16()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let mut buf = vec![0u8; len as usize];
    stream
        .read_exact(&mut buf)
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    String::from_utf8(buf).map_err(|_| TunnelError::Remote("invalid credential encoding".into()))
}

async fn read_server_ack(stream: &mut RecvStream) -> Result<(), TunnelError> {
    let ack = stream
        .read_u8()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    if ack == 1 {
        Ok(())
    } else {
        Err(TunnelError::Remote(
            "direct server rejected credential".into(),
        ))
    }
}

async fn write_server_ack(stream: &mut SendStream) -> Result<(), TunnelError> {
    stream
        .write_all(&[1u8])
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))
}
