use std::{
    collections::HashMap,
    io,
    net::SocketAddr,
    pin::Pin,
    sync::Arc,
    task::{Context, Poll},
};

use bytes::Bytes;
use h2::{client, server, RecvStream, SendStream};
use http::{
    header::{HeaderName, HeaderValue},
    HeaderMap, Request, Response,
};
use tokio::{
    io::{AsyncRead, AsyncWrite, ReadBuf},
    net::TcpStream,
    sync::RwLock,
};
use tracing::{debug, info, warn};
use uuid::Uuid;

const MAX_HTTP2_FALLBACK_BODY_BYTES: usize = 4 * 1024 * 1024;

use crate::{
    config::{DevboxConnection, RouteDecision, RoutingTable, SidecarSettings},
    http2_client::{ConnectResult, Http2ClientActor},
    otel_routing::{determine_routing_target, extract_routing_context, TRACESTATE_HEADER},
};

/// Handle HTTP/2 traffic by establishing server and client handshakes.
///
/// This initial version performs transparent proxying to the default local
/// destination. Future iterations will incorporate branch routing using HTTP/2
/// header inspection.
#[allow(clippy::too_many_arguments)]
pub async fn handle_http2_proxy(
    inbound_stream: TcpStream,
    client_addr: SocketAddr,
    original_dest: SocketAddr,
    initial_data: Vec<u8>,
    settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    _rpc_client: Arc<RwLock<Option<lapdev_kube_rpc::SidecarProxyManagerRpcClient>>>,
) -> io::Result<()> {
    let local_target = SocketAddr::new("127.0.0.1".parse().unwrap(), original_dest.port());

    debug!(
        "HTTP/2 proxy starting for {} -> {} (local target {})",
        client_addr, original_dest, local_target
    );

    // Connect to the local target. Routing decisions will be applied per stream.
    let outbound_stream = TcpStream::connect(&local_target).await?;

    // Replay any bytes already read during protocol detection before delegating
    // the rest of the connection to `h2`.
    let inbound_prefaced = PrefacedStream::new(inbound_stream, initial_data);
    let mut inbound_connection = server::handshake(inbound_prefaced)
        .await
        .map_err(map_h2_err)?;

    let (mut outbound_sender, outbound_connection) = client::handshake(outbound_stream)
        .await
        .map_err(map_h2_err)?;

    // Drive the outbound connection in the background so response frames and
    // settings are processed.
    tokio::spawn(async move {
        if let Err(err) = outbound_connection.await {
            warn!("Outbound HTTP/2 connection ended with error: {}", err);
        }
    });

    let _ = _rpc_client;
    let environment_id = settings.environment_id;

    while let Some(result) = inbound_connection.accept().await {
        let (request, mut respond) = match result {
            Ok(stream) => stream,
            Err(err) => {
                warn!(
                    "HTTP/2 accept error for {} -> {}: {}",
                    client_addr, original_dest, err
                );
                break;
            }
        };

        let routing_table = Arc::clone(&routing_table);

        if let Err(err) = proxy_http2_stream(
            &mut outbound_sender,
            request,
            &mut respond,
            client_addr,
            original_dest,
            environment_id,
            routing_table,
        )
        .await
        {
            warn!(
                "HTTP/2 stream proxy failure for {} -> {}: {}",
                client_addr, original_dest, err
            );
            break;
        }
    }

    Ok(())
}

async fn proxy_http2_stream(
    outbound_sender: &mut client::SendRequest<Bytes>,
    request: Request<RecvStream>,
    respond: &mut server::SendResponse<Bytes>,
    client_addr: SocketAddr,
    original_dest: SocketAddr,
    default_environment_id: Uuid,
    routing_table: Arc<RwLock<RoutingTable>>,
) -> io::Result<()> {
    let port = original_dest.port();
    let (mut parts, body) = request.into_parts();
    let mut body = ReplayableBody::new(body, MAX_HTTP2_FALLBACK_BODY_BYTES);

    let method = parts.method.clone();
    let path = parts.uri.path().to_string();
    let authority = parts
        .uri
        .authority()
        .map(|auth| auth.to_string())
        .unwrap_or_default();

    let header_pairs = header_map_to_vec(&parts.headers);
    let routing_context = extract_routing_context(&header_pairs);
    let branch_id = routing_context.lapdev_environment_id;
    let target_environment = branch_id.unwrap_or(default_environment_id);

    ensure_environment_tracestate(&mut parts.headers, target_environment)?;

    let decision = {
        let table = routing_table.read().await;
        table.resolve_http(port, branch_id)
    };

    match decision {
        RouteDecision::BranchService { service } => {
            if let Some(service_name) = service.service_name_for_port(port) {
                info!(
                    "HTTP/2 {} {} (authority {}) from {} routing to branch {:?} service {}:{}",
                    method, path, authority, client_addr, branch_id, service_name, port
                );

                let http2_clients = Arc::clone(&service.http2_clients);
                if let Err(err) = proxy_http2_branch_service(
                    service_name,
                    port,
                    http2_clients,
                    &parts,
                    &mut body,
                    respond,
                )
                .await
                {
                    warn!(
                        "HTTP/2 branch route {} for env {:?} failed: {}; falling back to shared target",
                        service_name, branch_id, err
                    );
                    if !body.supports_retry() {
                        warn!(
                            "HTTP/2 request from {} {} cannot be replayed for fallback",
                            method, path
                        );
                        return Err(io::Error::new(
                            io::ErrorKind::Other,
                            "request body cannot be replayed for fallback",
                        ));
                    }
                } else {
                    return Ok(());
                }
            } else {
                warn!(
                    "Missing branch service mapping for port {} in env {:?}; using shared target",
                    port, branch_id
                );
            }
        }
        RouteDecision::BranchDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata().clone();
            info!(
                "HTTP/2 {} {} (authority {}) from {} intercepted by branch devbox (env {:?}, intercept_id={}, session_id={}, target_port={})",
                method,
                path,
                authority,
                client_addr,
                branch_id,
                metadata.intercept_id,
                metadata.session_id,
                target_port
            );

            if let Err(err) = proxy_http2_devbox(
                Arc::clone(&connection),
                target_port,
                &parts,
                &mut body,
                respond,
            )
            .await
            {
                warn!(
                        "HTTP/2 branch devbox route intercept_id={} failed: {}; falling back to shared target",
                        metadata.intercept_id, err
                    );
                connection.clear_client().await;
                if !body.supports_retry() {
                    warn!(
                        "HTTP/2 request from {} {} cannot be replayed for fallback",
                        method, path
                    );
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "request body cannot be replayed for fallback",
                    ));
                }
            } else {
                return Ok(());
            }
        }
        RouteDecision::DefaultDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata().clone();
            info!(
                "HTTP/2 {} {} (authority {}) from {} intercepted by shared devbox (port {}, intercept_id={}, session_id={}, target_port={})",
                method,
                path,
                authority,
                client_addr,
                port,
                metadata.intercept_id,
                metadata.session_id,
                target_port
            );

            if let Err(err) = proxy_http2_devbox(
                Arc::clone(&connection),
                target_port,
                &parts,
                &mut body,
                respond,
            )
            .await
            {
                warn!(
                    "HTTP/2 shared devbox route intercept_id={} failed: {}; falling back to shared target",
                    metadata.intercept_id, err
                );
                connection.clear_client().await;
                if !body.supports_retry() {
                    warn!(
                        "HTTP/2 request from {} {} cannot be replayed for fallback",
                        method, path
                    );
                    return Err(io::Error::new(
                        io::ErrorKind::Other,
                        "request body cannot be replayed for fallback",
                    ));
                }
            } else {
                return Ok(());
            }
        }
        RouteDecision::DefaultLocal => {}
    }

    let routing_target = determine_routing_target(&routing_context, port);
    info!(
        "HTTP/2 {} {} (authority {}) from {} -> shared target (routing: {}, trace_id: {:?})",
        method,
        path,
        authority,
        client_addr,
        routing_target.get_metadata(),
        routing_context.trace_context.trace_id
    );

    forward_http2_via_sender(outbound_sender, &parts, &mut body, respond).await
}

async fn forward_http2_via_sender(
    sender: &mut client::SendRequest<Bytes>,
    parts: &http::request::Parts,
    body: &mut ReplayableBody,
    respond: &mut server::SendResponse<Bytes>,
) -> io::Result<()> {
    let outbound_request = build_outbound_request(parts)?;
    let end_of_stream = body.is_end_stream();

    let mut ready_sender = sender.clone().ready().await.map_err(map_h2_err)?;

    let (response_future, mut outbound_stream) = ready_sender
        .send_request(outbound_request, end_of_stream)
        .map_err(map_h2_err)?;
    *sender = ready_sender;

    if !end_of_stream {
        body.forward(&mut outbound_stream).await?;
    }

    let response = response_future.await.map_err(map_h2_err)?;
    let (response_parts, mut response_body) = response.into_parts();
    let response_end_of_stream = response_body.is_end_stream();
    let head = Response::from_parts(response_parts, ());
    let mut inbound_stream = respond
        .send_response(head, response_end_of_stream)
        .map_err(map_h2_err)?;

    if !response_end_of_stream {
        forward_response_body(&mut response_body, &mut inbound_stream).await?;
    }

    Ok(())
}

enum ReplayableBodyState {
    Streaming(RecvStream),
    Buffered,
    Unbuffered,
}

struct ReplayableBody {
    state: ReplayableBodyState,
    buffered_chunks: Vec<Bytes>,
    buffered_trailers: Option<HeaderMap>,
    initial_end_of_stream: bool,
    max_buffer_bytes: usize,
    total_buffered_bytes: usize,
    replayable: bool,
    overflow_warned: bool,
}

impl ReplayableBody {
    fn new(stream: RecvStream, max_buffer_bytes: usize) -> Self {
        let initial_end_of_stream = stream.is_end_stream();
        Self {
            state: ReplayableBodyState::Streaming(stream),
            buffered_chunks: Vec::new(),
            buffered_trailers: None,
            initial_end_of_stream,
            max_buffer_bytes,
            total_buffered_bytes: 0,
            replayable: true,
            overflow_warned: false,
        }
    }

    fn is_end_stream(&self) -> bool {
        self.initial_end_of_stream
    }

    fn supports_retry(&self) -> bool {
        if self.initial_end_of_stream {
            return true;
        }

        if !self.replayable {
            return false;
        }

        !matches!(self.state, ReplayableBodyState::Unbuffered)
    }

    async fn forward(&mut self, outbound_stream: &mut SendStream<Bytes>) -> io::Result<()> {
        if self.initial_end_of_stream {
            return Ok(());
        }

        for chunk in &self.buffered_chunks {
            outbound_stream
                .send_data(chunk.clone(), false)
                .map_err(map_h2_err)?;
        }

        match &mut self.state {
            ReplayableBodyState::Streaming(body) => {
                let mut trailers = None;
                {
                    let body_ref = body;
                    while let Some(chunk_result) = body_ref.data().await {
                        let data = chunk_result.map_err(map_h2_err)?;
                        let len = data.len();

                        if self.replayable {
                            let attempted_total = self.total_buffered_bytes + len;
                            if attempted_total > self.max_buffer_bytes {
                                self.replayable = false;
                                self.buffered_chunks.clear();
                                self.buffered_trailers = None;
                                self.total_buffered_bytes = 0;
                                if !self.overflow_warned {
                                    warn!(
                                        "HTTP/2 request body exceeded replay buffer ({} bytes > {}); fallback replay disabled",
                                        attempted_total,
                                        self.max_buffer_bytes
                                    );
                                    self.overflow_warned = true;
                                }
                            } else {
                                self.total_buffered_bytes += len;
                                self.buffered_chunks.push(data.clone());
                            }
                        }

                        outbound_stream.send_data(data, false).map_err(map_h2_err)?;
                        if let Err(err) = body_ref.flow_control().release_capacity(len) {
                            warn!("Failed to release HTTP/2 request capacity: {}", err);
                        }
                    }

                    trailers = body_ref.trailers().await.map_err(map_h2_err)?;
                }

                if let Some(trailers) = trailers {
                    if self.replayable {
                        self.buffered_trailers = Some(trailers.clone());
                    }
                    outbound_stream
                        .send_trailers(trailers)
                        .map_err(map_h2_err)?;
                } else {
                    outbound_stream
                        .send_data(Bytes::new(), true)
                        .map_err(map_h2_err)?;
                }

                self.state = if self.replayable {
                    ReplayableBodyState::Buffered
                } else {
                    ReplayableBodyState::Unbuffered
                };

                Ok(())
            }
            ReplayableBodyState::Buffered => {
                if let Some(trailers) = &self.buffered_trailers {
                    outbound_stream
                        .send_trailers(trailers.clone())
                        .map_err(map_h2_err)?;
                } else {
                    outbound_stream
                        .send_data(Bytes::new(), true)
                        .map_err(map_h2_err)?;
                }
                Ok(())
            }
            ReplayableBodyState::Unbuffered => Err(io::Error::new(
                io::ErrorKind::Other,
                "request body not replayable",
            )),
        }
    }
}

async fn proxy_http2_branch_service(
    service_name: &str,
    port: u16,
    clients: Arc<RwLock<HashMap<u16, Arc<Http2ClientActor>>>>,
    parts: &http::request::Parts,
    body: &mut ReplayableBody,
    respond: &mut server::SendResponse<Bytes>,
) -> io::Result<()> {
    let client = {
        let guard = clients.read().await;
        if let Some(existing) = guard.get(&port) {
            Arc::clone(existing)
        } else {
            drop(guard);

            let service_name_arc = Arc::new(service_name.to_string());
            let label = format!("branch-http2:{}:{}", service_name, port);
            let connect = Http2ClientActor::connector({
                let service_name = Arc::clone(&service_name_arc);
                move || {
                    let service_name = Arc::clone(&service_name);
                    async move {
                        let branch_stream =
                            TcpStream::connect((service_name.as_str(), port)).await?;
                        let (sender, connection) =
                            client::handshake(branch_stream).await.map_err(map_h2_err)?;
                        let driver = Box::pin(async move { connection.await });
                        Ok(ConnectResult::new(sender, driver))
                    }
                }
            });

            let mut guard = clients.write().await;
            guard
                .entry(port)
                .or_insert_with(|| Http2ClientActor::spawn(label, connect))
                .clone()
        }
    };

    let mut lease = match client.acquire().await {
        Ok(lease) => lease,
        Err(err) => {
            warn!(
                "Branch HTTP/2 acquire for {}:{} failed: {}",
                service_name, port, err
            );
            remove_branch_http2_client(&clients, port).await;
            return Err(err);
        }
    };

    match forward_http2_via_sender(lease.sender_mut(), parts, body, respond).await {
        Ok(()) => Ok(()),
        Err(err) => {
            lease.mark_broken();
            warn!(
                "HTTP/2 branch proxy to {}:{} failed mid-stream: {}",
                service_name, port, err
            );
            remove_branch_http2_client(&clients, port).await;
            Err(err)
        }
    }
}

async fn remove_branch_http2_client(
    clients: &Arc<RwLock<HashMap<u16, Arc<Http2ClientActor>>>>,
    port: u16,
) {
    let client = {
        let mut guard = clients.write().await;
        guard.remove(&port)
    };

    if let Some(client) = client {
        client.shutdown().await;
    }
}

async fn proxy_http2_devbox(
    connection: Arc<DevboxConnection>,
    target_port: u16,
    parts: &http::request::Parts,
    body: &mut ReplayableBody,
    respond: &mut server::SendResponse<Bytes>,
) -> io::Result<()> {
    let metadata = Arc::new(connection.metadata().clone());

    let client = connection
        .get_or_create_http2_client(target_port, {
            let connection = Arc::clone(&connection);
            let metadata = Arc::clone(&metadata);
            move || {
                let connection = Arc::clone(&connection);
                let metadata = Arc::clone(&metadata);
                let label = format!(
                    "devbox-http2:{}:{}",
                    metadata.intercept_id, target_port
                );
                let connect = Http2ClientActor::connector(move || {
                    let connection = Arc::clone(&connection);
                    let metadata = Arc::clone(&metadata);
                    async move {
                        let devbox_stream = match connection
                            .connect_tcp_stream("127.0.0.1", target_port)
                            .await
                        {
                            Ok(stream) => stream,
                            Err(err) => {
                                warn!(
                                    "Failed to connect to devbox intercept_id={} target_port={}: {}",
                                    metadata.intercept_id, target_port, err
                                );
                                connection.clear_client().await;
                                return Err(io::Error::from(err));
                            }
                        };

                        let (sender, devbox_connection) =
                            match client::handshake(devbox_stream).await {
                                Ok(pair) => pair,
                                Err(err) => {
                                    warn!(
                                        "HTTP/2 handshake with devbox intercept_id={} failed: {}",
                                        metadata.intercept_id, err
                                    );
                                    connection.clear_client().await;
                                    return Err(map_h2_err(err));
                                }
                            };

                        let driver = Box::pin(async move { devbox_connection.await });
                        Ok(ConnectResult::new(sender, driver))
                    }
                });

                Http2ClientActor::spawn(label, connect)
            }
        })
        .await;

    let mut lease = match client.acquire().await {
        Ok(lease) => lease,
        Err(err) => {
            warn!(
                "Devbox HTTP/2 sender unavailable for intercept_id={}, removing pooled client: {}",
                metadata.intercept_id, err
            );
            connection.remove_http2_client(target_port).await;
            return Err(err);
        }
    };

    match forward_http2_via_sender(lease.sender_mut(), parts, body, respond).await {
        Ok(()) => Ok(()),
        Err(err) => {
            lease.mark_broken();
            warn!(
                "HTTP/2 proxy via devbox intercept_id={} failed mid-stream: {}",
                metadata.intercept_id, err
            );
            connection.remove_http2_client(target_port).await;
            Err(err)
        }
    }
}

fn build_outbound_request(parts: &http::request::Parts) -> io::Result<Request<()>> {
    let mut builder = Request::builder()
        .method(&parts.method)
        .uri(&parts.uri)
        .version(parts.version);

    for (name, value) in parts.headers.iter() {
        builder = builder.header(name, value);
    }

    builder
        .body(())
        .map_err(|err| io::Error::new(io::ErrorKind::Other, err))
}

async fn forward_response_body(
    body: &mut RecvStream,
    inbound_stream: &mut SendStream<Bytes>,
) -> io::Result<()> {
    while let Some(chunk) = body.data().await {
        let data = chunk.map_err(map_h2_err)?;
        let len = data.len();
        inbound_stream.send_data(data, false).map_err(map_h2_err)?;
        if let Err(err) = body.flow_control().release_capacity(len) {
            warn!("Failed to release HTTP/2 response capacity: {}", err);
        }
    }

    if let Some(trailers) = body.trailers().await.map_err(map_h2_err)? {
        inbound_stream.send_trailers(trailers).map_err(map_h2_err)?;
    } else {
        inbound_stream
            .send_data(Bytes::new(), true)
            .map_err(map_h2_err)?;
    }

    Ok(())
}

fn header_map_to_vec(headers: &HeaderMap) -> Vec<(String, String)> {
    headers
        .iter()
        .map(|(name, value)| {
            let value_str = value
                .to_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|_| String::from_utf8_lossy(value.as_bytes()).to_string());
            (name.as_str().to_string(), value_str)
        })
        .collect()
}

fn ensure_environment_tracestate(headers: &mut HeaderMap, environment_id: Uuid) -> io::Result<()> {
    let env_id_str = environment_id.to_string();
    let target_entry = format!("lapdev-env-id={}", env_id_str);

    if let Some(existing_value) = headers.get(TRACESTATE_HEADER) {
        let existing = existing_value
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(existing_value.as_bytes()).to_string());

        let already_present = existing.split(',').any(|entry| {
            let trimmed = entry.trim();
            if let Some((key, value)) = trimmed.split_once('=') {
                key.eq_ignore_ascii_case("lapdev-env-id")
                    && value.trim().eq_ignore_ascii_case(&env_id_str)
            } else {
                false
            }
        });

        if already_present {
            return Ok(());
        }

        let new_value = if existing.trim().is_empty() {
            target_entry
        } else {
            format!("{},{}", existing.trim(), target_entry)
        };

        let header_value = HeaderValue::from_str(&new_value)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        headers.insert(HeaderName::from_static(TRACESTATE_HEADER), header_value);
    } else {
        let header_value = HeaderValue::from_str(&target_entry)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
        headers.insert(HeaderName::from_static(TRACESTATE_HEADER), header_value);
    }

    Ok(())
}

fn map_h2_err(err: h2::Error) -> io::Error {
    io::Error::new(io::ErrorKind::Other, err)
}

struct PrefacedStream<T> {
    buffer: Vec<u8>,
    position: usize,
    inner: T,
}

impl<T> PrefacedStream<T> {
    fn new(inner: T, buffer: Vec<u8>) -> Self {
        Self {
            buffer,
            position: 0,
            inner,
        }
    }
}

impl<T> AsyncRead for PrefacedStream<T>
where
    T: AsyncRead + Unpin,
{
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        if self.position < self.buffer.len() {
            let remaining = self.buffer.len() - self.position;
            let bytes_to_copy = remaining.min(buf.remaining());
            buf.put_slice(&self.buffer[self.position..self.position + bytes_to_copy]);
            self.position += bytes_to_copy;
            Poll::Ready(Ok(()))
        } else {
            Pin::new(&mut self.inner).poll_read(cx, buf)
        }
    }
}

impl<T> AsyncWrite for PrefacedStream<T>
where
    T: AsyncWrite + Unpin,
{
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.inner).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.inner).poll_shutdown(cx)
    }
}
