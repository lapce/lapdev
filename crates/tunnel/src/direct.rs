use std::{
    collections::HashSet,
    convert::TryFrom,
    io,
    net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, ToSocketAddrs, UdpSocket},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use lapdev_common::devbox::{
    DirectCandidate, DirectCandidateKind, DirectCandidateSet, DirectChannelConfig, DirectTransport,
};
use quinn::rustls;
use quinn::{self, Connection, Endpoint, IdleTimeout, RecvStream, SendStream, TransportConfig};
use rand::random;
use rcgen::generate_simple_self_signed;
use rustls::client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier};
use rustls::pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime};
use rustls::{DigitallySignedStruct, SignatureScheme};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    sync::RwLock,
    time::timeout,
};
use tracing::{debug, info, warn};

use crate::{
    client::{TunnelClient, TunnelMode},
    error::TunnelError,
};

const HANDSHAKE_MAX_TOKEN_LEN: usize = u16::MAX as usize;
const DIRECT_SERVER_NAME: &str = "lapdev.devbox";
const CERTIFICATE_ALT_NAMES: &[&str] = &["lapdev.devbox", "lapdev-direct", "lapdev-relay"];
const QUIC_KEEP_ALIVE_INTERVAL_SECS: u64 = 15;
const QUIC_IDLE_TIMEOUT_SECS: u64 = 60 * 60;
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// Options controlling direct QUIC server behavior.
#[derive(Debug, Clone)]
pub struct DirectServerOptions {
    /// STUN servers that should be queried to discover the server's reflexive address.
    pub stun_servers: Vec<SocketAddr>,
    /// How long to wait for a STUN response.
    pub stun_timeout: Duration,
}

impl Default for DirectServerOptions {
    fn default() -> Self {
        Self {
            stun_servers: Vec::new(),
            stun_timeout: Duration::from_millis(750),
        }
    }
}

/// Options that influence how direct transport candidates are collected.
#[derive(Debug, Clone, Default)]
pub struct CandidateOptions {
    /// Reflexive address reported by a STUN probe, if one was performed.
    pub stun_observed_addr: Option<SocketAddr>,
}

/// Collects direct transport candidates based on supplied options.
pub fn collect_candidates(opts: &CandidateOptions) -> DirectCandidateSet {
    let mut priority_counter = 0u32;
    let mut candidates = Vec::new();

    if let Some(addr) = opts.stun_observed_addr {
        let host = addr.ip().to_string();
        if !candidate_exists(&candidates, &host, addr.port()) {
            candidates.push(DirectCandidate {
                host,
                port: addr.port(),
                transport: DirectTransport::Quic,
                kind: DirectCandidateKind::Public,
                priority: next_priority(&mut priority_counter),
            });
        }
    }

    let generation = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|dur| dur.as_secs())
        .unwrap_or_default();

    DirectCandidateSet {
        candidates,
        generation: Some(generation),
        server_certificate: None,
    }
}

fn next_priority(counter: &mut u32) -> u32 {
    let value = *counter;
    *counter = counter.saturating_add(1);
    value
}

fn candidate_exists(candidates: &[DirectCandidate], host: &str, port: u16) -> bool {
    candidates
        .iter()
        .any(|candidate| candidate.host == host && candidate.port == port)
}

/// Attempt to establish a direct tunnel using the provided channel config.
pub async fn connect_direct_tunnel(
    config: &DirectChannelConfig,
) -> Result<TunnelClient, TunnelError> {
    match timeout(DIRECT_CONNECT_TIMEOUT, connect_direct_tunnel_inner(config)).await {
        Ok(result) => result,
        Err(_) => Err(TunnelError::Transport(io::Error::new(
            io::ErrorKind::TimedOut,
            format!(
                "direct tunnel attempt exceeded {}s",
                DIRECT_CONNECT_TIMEOUT.as_secs()
            ),
        ))),
    }
}

async fn connect_direct_tunnel_inner(
    config: &DirectChannelConfig,
) -> Result<TunnelClient, TunnelError> {
    let mut endpoint = Endpoint::client("[::]:0".parse().unwrap())
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    let server_cert = config
        .server_certificate
        .as_deref()
        .ok_or_else(|| TunnelError::Remote("direct server certificate missing".into()))?;
    endpoint.set_default_client_config(client_config_with_pinned_certificate(server_cert)?);

    let mut last_err: Option<TunnelError> = None;

    for candidate in config
        .candidates
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
            match endpoint.connect(addr, DIRECT_SERVER_NAME) {
                Ok(connecting) => match connecting.await {
                    Ok(connection) => {
                        match establish_client_connection(connection, &config.credential.token)
                            .await
                        {
                            Ok(transport) => {
                                info!(peer = %addr, "Direct QUIC tunnel established");
                                return Ok(TunnelClient::connect_with_mode(
                                    transport,
                                    TunnelMode::Direct,
                                ));
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

pub(crate) async fn establish_client_connection(
    connection: Connection,
    token: &str,
) -> Result<QuicTransport, TunnelError> {
    let (mut send, mut recv) = connection
        .open_bi()
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    write_token(&mut send, token).await?;
    read_server_ack(&mut recv).await?;
    send.finish()
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    Ok(QuicTransport::new(connection))
}

/// QUIC listener that validates the shared credential before handing over the transport.
pub struct DirectQuicServer {
    endpoint: Endpoint,
    credential: Arc<DirectCredentialStore>,
    observed_addr: Option<SocketAddr>,
    server_certificate: Vec<u8>,
}

#[derive(Default)]
pub struct DirectCredentialStore {
    tokens: RwLock<HashSet<String>>,
}

impl DirectCredentialStore {
    pub fn new() -> Self {
        Self {
            tokens: RwLock::new(HashSet::new()),
        }
    }

    pub async fn replace(&self, token: impl Into<String>) {
        let mut guard = self.tokens.write().await;
        guard.clear();
        guard.insert(token.into());
    }

    pub async fn insert(&self, token: impl Into<String>) {
        let mut guard = self.tokens.write().await;
        guard.insert(token.into());
    }

    pub async fn remove(&self, token: &str) -> bool {
        let mut guard = self.tokens.write().await;
        guard.remove(token)
    }

    pub async fn contains(&self, token: &str) -> bool {
        let guard = self.tokens.read().await;
        guard.contains(token)
    }
}

pub struct DirectConnection {
    pub transport: QuicTransport,
    pub token: String,
}

impl DirectQuicServer {
    pub async fn bind(bind_addr: SocketAddr) -> Result<Self, TunnelError> {
        Self::bind_with_options(bind_addr, DirectServerOptions::default()).await
    }

    pub async fn bind_with_options(
        bind_addr: SocketAddr,
        options: DirectServerOptions,
    ) -> Result<Self, TunnelError> {
        let (server_config, server_certificate) = build_server_config()?;
        let socket = std::net::UdpSocket::bind(bind_addr)
            .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
        socket
            .set_nonblocking(false)
            .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;

        let observed_addr = if options.stun_servers.is_empty() {
            None
        } else {
            match run_stun_probes(&socket, &options.stun_servers, options.stun_timeout) {
                Ok(addr) => addr,
                Err(err) => {
                    warn!(error = %err, "STUN probe failed");
                    None
                }
            }
        };

        socket
            .set_nonblocking(true)
            .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
        let runtime = quinn::default_runtime()
            .ok_or_else(|| TunnelError::Transport(io::Error::other("no async runtime found")))?;
        let endpoint = Endpoint::new(
            quinn::EndpointConfig::default(),
            Some(server_config),
            socket,
            runtime,
        )
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
        Ok(Self {
            endpoint,
            credential: Arc::new(DirectCredentialStore::new()),
            observed_addr,
            server_certificate,
        })
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TunnelError> {
        self.endpoint
            .local_addr()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))
    }

    pub fn credential_handle(&self) -> Arc<DirectCredentialStore> {
        Arc::clone(&self.credential)
    }

    pub fn observed_addr(&self) -> Option<SocketAddr> {
        self.observed_addr
    }

    pub fn server_certificate(&self) -> &[u8] {
        &self.server_certificate
    }

    pub async fn accept(&self) -> Result<DirectConnection, TunnelError> {
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

        let presented = read_token(&mut recv).await?;
        if !self.credential.contains(&presented).await {
            return Err(TunnelError::Remote("invalid direct credential".into()));
        }

        write_server_ack(&mut send).await?;
        send.finish()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

        Ok(DirectConnection {
            transport: QuicTransport::new(connection),
            token: presented,
        })
    }

    pub fn close(&self) {
        self.endpoint.close(0u32.into(), b"direct server shutdown");
    }
}

/// Handle to an authenticated QUIC connection that can open new streams.
pub struct QuicTransport {
    connection: Connection,
}

impl QuicTransport {
    pub(crate) fn new(connection: Connection) -> Self {
        Self { connection }
    }

    pub fn connection(&self) -> &Connection {
        &self.connection
    }

    pub fn into_connection(self) -> Connection {
        self.connection
    }
}

pub(crate) fn build_server_config() -> Result<(quinn::ServerConfig, Vec<u8>), TunnelError> {
    let names: Vec<String> = CERTIFICATE_ALT_NAMES
        .iter()
        .map(|name| name.to_string())
        .collect();
    let rcgen::CertifiedKey { cert, signing_key } = generate_simple_self_signed(names)
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    let cert_der_bytes = cert.der().as_ref().to_vec();
    let cert_chain = vec![CertificateDer::from(cert)];
    let private_key = PrivatePkcs8KeyDer::from(signing_key.serialize_der());

    let mut config = quinn::ServerConfig::with_single_cert(cert_chain, private_key.into())
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
    config.transport_config(tunnel_transport_config()?);
    Ok((config, cert_der_bytes))
}

pub(crate) fn client_config_with_pinned_certificate(
    server_certificate: &[u8],
) -> Result<quinn::ClientConfig, TunnelError> {
    let mut roots = rustls::RootCertStore::empty();
    roots
        .add(CertificateDer::from(server_certificate.to_vec()))
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

    let mut crypto = rustls::ClientConfig::builder()
        .with_root_certificates(Arc::new(roots))
        .with_no_client_auth();
    crypto.enable_early_data = true;

    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(crypto))
        .expect("invalid QUIC client crypto config");
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_crypto));
    client_config.transport_config(tunnel_transport_config()?);
    Ok(client_config)
}

pub(crate) fn client_config() -> Result<quinn::ClientConfig, TunnelError> {
    let roots = Arc::new(rustls::RootCertStore::empty());
    let mut crypto = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    crypto
        .dangerous()
        .set_certificate_verifier(Arc::new(SkipServerVerification));
    crypto.enable_early_data = true;

    let quic_crypto = quinn::crypto::rustls::QuicClientConfig::try_from(Arc::new(crypto))
        .expect("invalid QUIC client crypto config");
    let mut client_config = quinn::ClientConfig::new(Arc::new(quic_crypto));
    client_config.transport_config(tunnel_transport_config()?);
    Ok(client_config)
}

fn tunnel_transport_config() -> Result<Arc<TransportConfig>, TunnelError> {
    let mut transport = TransportConfig::default();
    transport.keep_alive_interval(Some(Duration::from_secs(QUIC_KEEP_ALIVE_INTERVAL_SECS)));
    transport.max_idle_timeout(Some(
        IdleTimeout::try_from(Duration::from_secs(QUIC_IDLE_TIMEOUT_SECS))
            .map_err(|err| TunnelError::Transport(io::Error::other(err)))?,
    ));
    Ok(Arc::new(transport))
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

pub(crate) async fn write_token(stream: &mut SendStream, token: &str) -> Result<(), TunnelError> {
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

pub(crate) async fn read_token(stream: &mut RecvStream) -> Result<String, TunnelError> {
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

pub(crate) async fn read_server_ack(stream: &mut RecvStream) -> Result<(), TunnelError> {
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

pub(crate) async fn write_server_ack(stream: &mut SendStream) -> Result<(), TunnelError> {
    stream
        .write_all(&[1u8])
        .await
        .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))
}

const STUN_MAGIC_COOKIE: u32 = 0x2112_A442;
const STUN_HEADER_SIZE: usize = 20;
const STUN_BINDING_REQUEST: u16 = 0x0001;
const STUN_BINDING_SUCCESS: u16 = 0x0101;
const STUN_ATTR_MAPPED_ADDRESS: u16 = 0x0001;
const STUN_ATTR_XOR_MAPPED_ADDRESS: u16 = 0x0020;

fn run_stun_probes(
    socket: &UdpSocket,
    servers: &[SocketAddr],
    timeout: Duration,
) -> io::Result<Option<SocketAddr>> {
    if servers.is_empty() {
        return Ok(None);
    }

    socket.set_read_timeout(Some(timeout))?;
    socket.set_write_timeout(Some(timeout))?;

    let mut result = Ok(None);
    for server in servers {
        match send_stun_binding_request(socket, *server) {
            Ok(Some(addr)) => {
                debug!(?server, observed = %addr, "STUN reported public address");
                result = Ok(Some(addr));
                break;
            }
            Ok(None) => {
                debug!(?server, "STUN response missing mapped address attribute");
            }
            Err(err)
                if matches!(
                    err.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) =>
            {
                debug!(?server, "STUN probe timed out");
            }
            Err(err) => {
                warn!(?server, error = %err, "STUN probe error");
            }
        }
    }

    if let Err(err) = socket.set_read_timeout(None) {
        debug!(error = %err, "Failed to reset STUN read timeout");
    }
    if let Err(err) = socket.set_write_timeout(None) {
        debug!(error = %err, "Failed to reset STUN write timeout");
    }

    result
}

fn send_stun_binding_request(
    socket: &UdpSocket,
    server: SocketAddr,
) -> io::Result<Option<SocketAddr>> {
    let transaction_id: [u8; 12] = random();

    let mut request = [0u8; STUN_HEADER_SIZE];
    request[0] = (STUN_BINDING_REQUEST >> 8) as u8;
    request[1] = STUN_BINDING_REQUEST as u8;
    request[2] = 0;
    request[3] = 0;
    request[4..8].copy_from_slice(&STUN_MAGIC_COOKIE.to_be_bytes());
    request[8..20].copy_from_slice(&transaction_id);

    socket.send_to(&request, server)?;

    let mut buf = [0u8; 576];
    let (len, _) = socket.recv_from(&mut buf)?;
    parse_stun_response(&buf[..len], &transaction_id)
}

fn parse_stun_response(data: &[u8], transaction_id: &[u8; 12]) -> io::Result<Option<SocketAddr>> {
    if data.len() < STUN_HEADER_SIZE {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "STUN response too short",
        ));
    }

    let message_type = u16::from_be_bytes([data[0], data[1]]);
    if message_type != STUN_BINDING_SUCCESS {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected STUN response type",
        ));
    }

    let message_len = u16::from_be_bytes([data[2], data[3]]) as usize;
    if data.len() < STUN_HEADER_SIZE + message_len {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "STUN response truncated",
        ));
    }

    if data[4..8] != STUN_MAGIC_COOKIE.to_be_bytes() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "invalid STUN magic cookie",
        ));
    }

    if data[8..20] != transaction_id[..] {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "mismatched STUN transaction id",
        ));
    }

    let mut offset = STUN_HEADER_SIZE;
    let end = STUN_HEADER_SIZE + message_len;
    while offset + 4 <= end {
        let attr_type = u16::from_be_bytes([data[offset], data[offset + 1]]);
        let attr_len = u16::from_be_bytes([data[offset + 2], data[offset + 3]]) as usize;
        let value_start = offset + 4;
        let value_end = value_start + attr_len;
        if value_end > data.len() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "STUN attribute exceeds message length",
            ));
        }

        let value = &data[value_start..value_end];
        match attr_type {
            STUN_ATTR_XOR_MAPPED_ADDRESS => {
                if let Some(addr) = parse_xor_mapped_address(value, transaction_id) {
                    return Ok(Some(addr));
                }
            }
            STUN_ATTR_MAPPED_ADDRESS => {
                if let Some(addr) = parse_mapped_address(value) {
                    return Ok(Some(addr));
                }
            }
            _ => {}
        }

        let padding = (4 - (attr_len % 4)) % 4;
        offset = value_end + padding;
    }

    Ok(None)
}

fn parse_mapped_address(value: &[u8]) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let family = value[1];
    let port = u16::from_be_bytes([value[2], value[3]]);
    match family {
        0x01 => {
            if value.len() < 8 {
                return None;
            }
            let addr = Ipv4Addr::new(value[4], value[5], value[6], value[7]);
            Some(SocketAddr::new(IpAddr::V4(addr), port))
        }
        0x02 => {
            if value.len() < 20 {
                return None;
            }
            let mut bytes = [0u8; 16];
            bytes.copy_from_slice(&value[4..20]);
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(bytes)), port))
        }
        _ => None,
    }
}

fn parse_xor_mapped_address(value: &[u8], transaction_id: &[u8; 12]) -> Option<SocketAddr> {
    if value.len() < 4 {
        return None;
    }
    let family = value[1];
    let mut port = u16::from_be_bytes([value[2], value[3]]);
    port ^= (STUN_MAGIC_COOKIE >> 16) as u16;
    match family {
        0x01 => {
            if value.len() < 8 {
                return None;
            }
            let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
            let mut addr = [0u8; 4];
            for i in 0..4 {
                addr[i] = value[4 + i] ^ cookie[i];
            }
            Some(SocketAddr::new(
                IpAddr::V4(Ipv4Addr::new(addr[0], addr[1], addr[2], addr[3])),
                port,
            ))
        }
        0x02 => {
            if value.len() < 20 {
                return None;
            }
            let cookie = STUN_MAGIC_COOKIE.to_be_bytes();
            let mut addr = [0u8; 16];
            for i in 0..16 {
                let xor_byte = if i < 4 {
                    cookie[i]
                } else {
                    transaction_id[i - 4]
                };
                addr[i] = value[4 + i] ^ xor_byte;
            }
            Some(SocketAddr::new(IpAddr::V6(Ipv6Addr::from(addr)), port))
        }
        _ => None,
    }
}
