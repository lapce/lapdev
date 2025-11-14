use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use lapdev_common::devbox::DirectTunnelConfig;
use quinn::{self, AsyncUdpSocket, Connection, Endpoint, Incoming};
use tokio::{net::UdpSocket as TokioUdpSocket, time::timeout};
use tracing::{debug, info, warn};

use crate::{client::TunnelClient, error::TunnelError, run_tunnel_server};

use super::{
    credential::{start_credential_cleanup, CredentialCleanupHandle, DirectCredentialStore},
    handshake::{read_server_ack, read_token, write_server_ack, write_token},
    probe,
    stun::{default_stun_servers, run_stun_probes, start_stun_keepalive, StunKeepaliveHandle},
    transport::{build_server_config, client_config_with_pinned_certificate},
    udp::DirectUdpSocket,
};

const DIRECT_SERVER_NAME: &str = "lapdev.devbox";
const DIRECT_CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const DEFAULT_STUN_KEEPALIVE_INTERVAL: Duration = Duration::from_secs(20);
const MAX_PROBES: usize = 6;
const PROBE_INTERVAL_MS: u64 = 200;

/// Options controlling direct QUIC server behavior.
#[derive(Debug, Clone)]
pub struct DirectEndpointOptions {
    /// STUN servers that should be queried to discover the server's reflexive address.
    pub stun_servers: Vec<SocketAddr>,
    /// How long to wait for a STUN response.
    pub stun_timeout: Duration,
    /// How often to send STUN keepalive probes.
    pub stun_keepalive_interval: Option<Duration>,
}

impl Default for DirectEndpointOptions {
    fn default() -> Self {
        Self {
            stun_servers: Vec::new(),
            stun_timeout: Duration::from_millis(750),
            stun_keepalive_interval: Some(DEFAULT_STUN_KEEPALIVE_INTERVAL),
        }
    }
}

/// Wrapper that pairs a Quinn endpoint with NAT discovery metadata.
#[derive(Clone)]
pub struct DirectEndpoint {
    endpoint: Endpoint,
    observed_addr: Arc<Mutex<Option<SocketAddr>>>,
    server_certificate: Arc<Vec<u8>>,
    credential: Arc<DirectCredentialStore>,
    hole_punch_socket: Arc<TokioUdpSocket>,
    stun_keepalive: Arc<Mutex<Option<StunKeepaliveHandle>>>,
    credential_cleanup: Arc<Mutex<Option<CredentialCleanupHandle>>>,
}

impl DirectEndpoint {
    /// Bind a new endpoint on 0.0.0.0:0 using the default STUN servers.
    pub async fn bind() -> Result<Self, TunnelError> {
        Self::bind_on(([0, 0, 0, 0], 0).into()).await
    }

    /// Bind on the provided address using the default STUN servers.
    pub async fn bind_on(bind_addr: SocketAddr) -> Result<Self, TunnelError> {
        let options = DirectEndpointOptions {
            stun_servers: default_stun_servers(),
            ..Default::default()
        };
        Self::bind_with_options(bind_addr, options).await
    }

    /// Bind using custom STUN options with the default Quinn configuration.
    pub async fn bind_with_options(
        bind_addr: SocketAddr,
        options: DirectEndpointOptions,
    ) -> Result<Self, TunnelError> {
        Self::bind_with_quinn_config(bind_addr, quinn::EndpointConfig::default(), None, options)
            .await
    }

    /// Bind with custom Quinn configuration and STUN options.
    pub async fn bind_with_quinn_config(
        bind_addr: SocketAddr,
        endpoint_config: quinn::EndpointConfig,
        server_config: Option<(quinn::ServerConfig, Vec<u8>)>,
        options: DirectEndpointOptions,
    ) -> Result<Self, TunnelError> {
        let socket = TokioUdpSocket::bind(bind_addr)
            .await
            .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
        Self::from_bound_socket(socket, endpoint_config, server_config, options).await
    }

    /// Construct from an already-bound socket, applying the provided Quinn configuration.
    pub async fn from_bound_socket(
        socket: TokioUdpSocket,
        endpoint_config: quinn::EndpointConfig,
        server_config: Option<(quinn::ServerConfig, Vec<u8>)>,
        options: DirectEndpointOptions,
    ) -> Result<Self, TunnelError> {
        let DirectEndpointOptions {
            stun_servers,
            stun_timeout,
            stun_keepalive_interval,
        } = options;

        let keepalive_interval = stun_keepalive_interval;
        let keepalive_servers = stun_servers.clone();

        let direct_udp_socket = Arc::new(
            DirectUdpSocket::new(socket)
                .map_err(|err| TunnelError::Transport(io::Error::other(err)))?,
        );
        let hole_punch_socket = direct_udp_socket.io();

        let (server_config, server_certificate) = match server_config {
            Some((config, cert)) => (config, Arc::new(cert)),
            None => {
                let (config, cert) = build_server_config()?;
                (config, Arc::new(cert))
            }
        };

        let runtime = quinn::default_runtime()
            .ok_or_else(|| TunnelError::Transport(io::Error::other("no async runtime found")))?;
        let endpoint = Endpoint::new_with_abstract_socket(
            endpoint_config,
            Some(server_config),
            Arc::clone(&direct_udp_socket) as Arc<dyn AsyncUdpSocket>,
            runtime,
        )
        .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;

        let initial_observed_addr = if stun_servers.is_empty() {
            None
        } else {
            match run_stun_probes(Arc::clone(&direct_udp_socket), &stun_servers, stun_timeout).await
            {
                Ok(addr) => addr,
                Err(err) => {
                    warn!(error = %err, "STUN probe failed");
                    None
                }
            }
        };
        let observed_addr = Arc::new(Mutex::new(initial_observed_addr));

        let stun_keepalive = match keepalive_interval {
            Some(interval) if !keepalive_servers.is_empty() => Some(start_stun_keepalive(
                Arc::clone(&direct_udp_socket),
                keepalive_servers,
                interval,
                stun_timeout,
                Arc::clone(&observed_addr),
            )),
            _ => None,
        };

        let credential = Arc::new(DirectCredentialStore::new());
        let cleanup_handle = start_credential_cleanup(Arc::clone(&credential));

        Ok(Self {
            endpoint,
            observed_addr,
            server_certificate,
            credential,
            hole_punch_socket,
            stun_keepalive: Arc::new(Mutex::new(stun_keepalive)),
            credential_cleanup: Arc::new(Mutex::new(Some(cleanup_handle))),
        })
    }

    pub fn endpoint(&self) -> Endpoint {
        self.endpoint.clone()
    }

    pub fn observed_addr(&self) -> Option<SocketAddr> {
        *self
            .observed_addr
            .lock()
            .expect("observed addr mutex poisoned")
    }

    pub fn server_certificate(&self) -> &[u8] {
        self.server_certificate.as_slice()
    }

    pub fn credential(&self) -> &DirectCredentialStore {
        &self.credential
    }

    /// Send a STUN-like UDP probe to the provided address to prime NAT state.
    pub async fn send_probe(&self, addr: SocketAddr, client: bool) -> Result<(), TunnelError> {
        let mut probes_sent = 0usize;
        loop {
            let payload = if client {
                probe::build_request_probe_payload()
            } else {
                probe::build_response_probe_payload(addr)
            };
            self.hole_punch_socket
                .send_to(&payload, addr)
                .await
                .map(|_| ())
                .map_err(|err| TunnelError::Transport(io::Error::other(err)))?;
            probes_sent += 1;
            if probes_sent >= MAX_PROBES {
                break;
            }
            tokio::time::sleep(Duration::from_millis(PROBE_INTERVAL_MS)).await;
        }

        Ok(())
    }

    pub async fn connect_tunnel(
        &self,
        config: &DirectTunnelConfig,
    ) -> Result<TunnelClient, TunnelError> {
        match timeout(DIRECT_CONNECT_TIMEOUT, self.connect_tunnel_inner(config)).await {
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

    async fn connect_tunnel_inner(
        &self,
        config: &DirectTunnelConfig,
    ) -> Result<TunnelClient, TunnelError> {
        let server_cert = config
            .server_certificate
            .as_deref()
            .ok_or_else(|| TunnelError::Remote("direct server certificate missing".into()))?;
        let client_config = client_config_with_pinned_certificate(server_cert)?;
        let target_addr = config
            .stun_observed_addr
            .ok_or_else(|| TunnelError::Remote("direct server STUN address unavailable".into()))?;

        {
            // let endpoint = self.clone();
            // tokio::spawn(async move {
            // let _ = endpoint.send_probe(target_addr, true).await;
            // });
        }

        debug!(candidate = %target_addr, "Attempting direct QUIC connect via STUN address");
        let connecting = self
            .endpoint
            .connect_with(client_config, target_addr, DIRECT_SERVER_NAME)
            .map_err(|err| {
                warn!(
                    candidate = %target_addr,
                    error = %err,
                    "Unable to initiate direct QUIC connect"
                );
                TunnelError::Transport(std::io::Error::other(err))
            })?;

        match connecting.await {
            Ok(connection) => {
                match self
                    .establish_client_connection(connection, &config.credential.token)
                    .await
                {
                    Ok(connection) => {
                        info!(peer = %target_addr, "Direct QUIC tunnel established");
                        Ok(TunnelClient::new_direct(connection))
                    }
                    Err(err) => {
                        warn!(
                            candidate = %target_addr,
                            error = %err,
                            "Direct QUIC handshake failed"
                        );
                        Err(err)
                    }
                }
            }
            Err(err) => {
                warn!(
                    candidate = %target_addr,
                    error = %err,
                    "Direct QUIC connect failed"
                );
                Err(TunnelError::Transport(std::io::Error::other(err)))
            }
        }
    }

    async fn establish_client_connection(
        &self,
        connection: Connection,
        token: &str,
    ) -> Result<Connection, TunnelError> {
        let (mut send, mut recv) = connection
            .open_bi()
            .await
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
        write_token(&mut send, token).await?;
        read_server_ack(&mut recv).await?;
        send.finish()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
        Ok(connection)
    }

    pub fn local_addr(&self) -> Result<SocketAddr, TunnelError> {
        self.endpoint
            .local_addr()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))
    }

    pub fn close(&self) {
        self.endpoint
            .close(0u32.into(), b"direct endpoint shutdown");
        self.stop_stun_keepalive();
        self.stop_credential_cleanup();
    }

    fn stop_stun_keepalive(&self) {
        if let Some(handle) = self
            .stun_keepalive
            .lock()
            .expect("stun keepalive mutex poisoned")
            .take()
        {
            handle.stop();
        }
    }

    fn stop_credential_cleanup(&self) {
        if let Some(handle) = self
            .credential_cleanup
            .lock()
            .expect("credential cleanup mutex poisoned")
            .take()
        {
            handle.stop();
        }
    }

    pub async fn start_tunnel_server(&self) {
        let local_addr = self.endpoint.local_addr().ok();
        let observed_addr = self.observed_addr();
        info!(
            ?local_addr,
            ?observed_addr,
            "Direct tunnel server listening for incoming peers"
        );
        while let Some(incoming) = self.endpoint.accept().await {
            debug!("Direct endpoint received incoming QUIC connection");
            let endpoint = self.clone();
            tokio::spawn(async move {
                if let Err(err) = endpoint.handle_incoming(incoming).await {
                    warn!(error = %err, "Direct tunnel connection ended with error");
                }
            });
        }
        warn!("Direct endpoint stopped accepting incoming connections");
    }

    async fn handle_incoming(&self, incoming: Incoming) -> Result<(), TunnelError> {
        let connecting = incoming
            .accept()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

        let connection = connecting
            .await
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;
        let remote_addr = connection.remote_address();
        let (mut send, mut recv) = connection
            .accept_bi()
            .await
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

        let presented = read_token(&mut recv).await?;
        if !self.credential.remove(&presented).await {
            warn!(?remote_addr, "Direct endpoint rejected invalid credential");
            return Err(TunnelError::Remote("invalid direct credential".into()));
        }

        debug!(
            ?remote_addr,
            "Direct endpoint authenticated peer credential"
        );
        write_server_ack(&mut send).await?;
        send.finish()
            .map_err(|err| TunnelError::Transport(std::io::Error::other(err)))?;

        info!(?remote_addr, "Direct endpoint established tunnel session");
        run_tunnel_server(connection).await?;

        Ok(())
    }
}

impl Drop for DirectEndpoint {
    fn drop(&mut self) {
        self.stop_stun_keepalive();
        self.stop_credential_cleanup();
    }
}
