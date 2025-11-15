use std::{
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

use crate::{
    config::{DevboxConnection, RouteDecision, RoutingTable, SidecarSettings},
    connection_registry::ConnectionRegistry,
    otel_routing::{extract_routing_context, TRACESTATE_HEADER},
};

/// Connection-level routing plan derived from the first HTTP/2 stream.
struct ConnectionRoutePlan {
    upstream: UpstreamConnector,
    service_port: u16,
    branch_id: Option<Uuid>,
    target_environment_id: Uuid,
    description: String,
}

/// Represents how the sidecar should connect upstream for the lifetime of a
/// single HTTP/2 connection from the workload.
enum UpstreamConnector {
    Local {
        target: SocketAddr,
    },
    BranchService {
        service_name: String,
        port: u16,
    },
    Devbox {
        connection: Arc<DevboxConnection>,
        target_port: u16,
        label: &'static str,
    },
}

#[derive(Clone)]
struct ConnectionContext {
    client_addr: SocketAddr,
    proxy_port: u16,
    service_port: u16,
    branch_id: Option<Uuid>,
    description: Arc<str>,
    target_environment_id: Uuid,
}

/// Handle an inbound HTTP/2 connection using connection-level routing. Once we
/// inspect the first stream to determine the routing target, all subsequent
/// streams on the same connection are forwarded to that same upstream.
#[allow(clippy::too_many_arguments)]
pub async fn handle_http2_proxy(
    inbound_stream: TcpStream,
    client_addr: SocketAddr,
    proxy_port: u16,
    initial_data: Vec<u8>,
    settings: Arc<SidecarSettings>,
    routing_table: Arc<RwLock<RoutingTable>>,
    _rpc_client: Arc<RwLock<Option<lapdev_kube_rpc::SidecarProxyManagerRpcClient>>>,
    connection_registry: Arc<ConnectionRegistry>,
) -> io::Result<()> {
    // Prepare the inbound HTTP/2 connection (server side).
    let inbound_prefaced = PrefacedStream::new(inbound_stream, initial_data);
    let mut inbound_connection = server::handshake(inbound_prefaced)
        .await
        .map_err(map_h2_err)?;

    // We need the first stream to determine the routing decision for the whole
    // connection. If the client closes the connection before sending a stream,
    // we can exit early.
    let first_stream = match inbound_connection.accept().await {
        Some(Ok(stream)) => stream,
        Some(Err(err)) => {
            warn!(
                "HTTP/2 accept error for {} on proxy port {}: {}",
                client_addr, proxy_port, err
            );
            return Err(map_h2_err(err));
        }
        None => return Ok(()),
    };

    let route_plan = determine_connection_route(
        &first_stream,
        proxy_port,
        Arc::clone(&routing_table),
        settings.environment_id,
    )
    .await?;

    debug!(
        target = %route_plan.description,
        service_port = route_plan.service_port,
        branch = ?route_plan.branch_id,
        "HTTP/2 connection from {} (proxy port {}) routed",
        client_addr,
        proxy_port
    );

    let (outbound_sender, _driver_task) = establish_upstream_connection(&route_plan).await?;

    let connection_context = Arc::new(ConnectionContext {
        client_addr,
        proxy_port,
        service_port: route_plan.service_port,
        branch_id: route_plan.branch_id,
        description: Arc::from(route_plan.description.into_boxed_str()),
        target_environment_id: route_plan.target_environment_id,
    });

    // Process the first stream immediately using the newly established upstream.
    let mut stream_sender = outbound_sender.clone();
    let (first_request, mut first_respond) = first_stream;
    let context_clone = Arc::clone(&connection_context);
    tokio::spawn(async move {
        if let Err(err) = proxy_http2_stream(
            &mut stream_sender,
            first_request,
            &mut first_respond,
            context_clone,
        )
        .await
        {
            warn!("HTTP/2 stream proxy failure: {}", err);
        }
    });

    let mut shutdown_rx = connection_registry.subscribe(None).await;

    // Forward the remaining streams (if any) to the same upstream target.
    loop {
        tokio::select! {
            biased;
            recv = shutdown_rx.changed() => {
                if recv.is_ok() || recv.is_err() {
                    info!(
                        "HTTP/2 connection for {} on proxy port {} closing due to routing update",
                        client_addr, proxy_port
                    );
                    break;
                }
            }
            result = inbound_connection.accept() => {
                let Some(result) = result else {
                    break;
                };

                let (request, mut respond) = match result {
                    Ok(stream) => stream,
                    Err(err) => {
                        warn!(
                            "HTTP/2 accept error for {} on proxy port {}: {}",
                            client_addr, proxy_port, err
                        );
                        break;
                    }
                };

                let mut stream_sender = outbound_sender.clone();
                let context_clone = Arc::clone(&connection_context);
                tokio::spawn(async move {
                    if let Err(err) = proxy_http2_stream(
                        &mut stream_sender,
                        request,
                        &mut respond,
                        context_clone,
                    )
                    .await
                    {
                        warn!("HTTP/2 stream proxy failure: {}", err);
                    }
                });
            }
        }
    }

    Ok(())
}

async fn determine_connection_route(
    first_stream: &(Request<RecvStream>, server::SendResponse<Bytes>),
    proxy_port: u16,
    routing_table: Arc<RwLock<RoutingTable>>,
    default_environment_id: Uuid,
) -> io::Result<ConnectionRoutePlan> {
    let (request, _) = first_stream;
    let header_pairs = header_map_to_vec(request.headers());
    let routing_context = extract_routing_context(&header_pairs);
    let branch_id = routing_context.lapdev_environment_id;
    let target_environment_id = branch_id.unwrap_or(default_environment_id);

    let (service_port, decision) = {
        let table = routing_table.read().await;
        let service_port = table.service_port_for_proxy(proxy_port);
        let decision = table.resolve_http(service_port, branch_id);
        (service_port, decision)
    };

    let (upstream, description) = match decision {
        RouteDecision::BranchService { service } => {
            let Some(service_name) = service.service_name_for_port(service_port) else {
                return Err(io::Error::new(
                    io::ErrorKind::Other,
                    format!(
                        "missing branch service mapping for port {} (branch {:?})",
                        service_port, branch_id
                    ),
                ));
            };

            let description = format!(
                "branch service {}:{} (branch {:?})",
                service_name, service_port, branch_id
            );

            (
                UpstreamConnector::BranchService {
                    service_name: service_name.to_string(),
                    port: service_port,
                },
                description,
            )
        }
        RouteDecision::BranchDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata().clone();
            let description = format!(
                "branch devbox intercept_id={} workload_id={} target_port={} (branch {:?})",
                metadata.intercept_id, metadata.workload_id, target_port, branch_id
            );
            (
                UpstreamConnector::Devbox {
                    connection,
                    target_port,
                    label: "branch-devbox",
                },
                description,
            )
        }
        RouteDecision::DefaultDevbox {
            connection,
            target_port,
        } => {
            let metadata = connection.metadata().clone();
            let description = format!(
                "shared devbox intercept_id={} workload_id={} target_port={}",
                metadata.intercept_id, metadata.workload_id, target_port
            );
            (
                UpstreamConnector::Devbox {
                    connection,
                    target_port,
                    label: "shared-devbox",
                },
                description,
            )
        }
        RouteDecision::DefaultLocal { target_port } => {
            let target = SocketAddr::new("127.0.0.1".parse().unwrap(), target_port);
            let description = format!("shared target {}", target);
            (UpstreamConnector::Local { target }, description)
        }
    };

    Ok(ConnectionRoutePlan {
        upstream,
        service_port,
        branch_id,
        target_environment_id,
        description,
    })
}

async fn establish_upstream_connection(
    plan: &ConnectionRoutePlan,
) -> io::Result<(client::SendRequest<Bytes>, tokio::task::JoinHandle<()>)> {
    match &plan.upstream {
        UpstreamConnector::Local { target } => {
            let stream = TcpStream::connect(target).await?;
            spawn_upstream_driver(client::handshake(stream).await.map_err(map_h2_err)?, plan)
        }
        UpstreamConnector::BranchService { service_name, port } => {
            let stream = TcpStream::connect((service_name.as_str(), *port)).await?;
            spawn_upstream_driver(client::handshake(stream).await.map_err(map_h2_err)?, plan)
        }
        UpstreamConnector::Devbox {
            connection,
            target_port,
            label,
        } => match connection
            .connect_tcp_stream("127.0.0.1", *target_port)
            .await
        {
            Ok(stream) => match client::handshake(stream).await {
                Ok(res) => spawn_upstream_driver(res, plan),
                Err(err) => {
                    warn!("HTTP/2 handshake with {} failed: {}", label, err);
                    Err(map_h2_err(err))
                }
            },
            Err(err) => {
                warn!(
                    "Failed to connect to {} target_port={}: {}",
                    label, target_port, err
                );
                Err(err)
            }
        },
    }
}

fn spawn_upstream_driver<T>(
    pair: (client::SendRequest<Bytes>, client::Connection<T, Bytes>),
    plan: &ConnectionRoutePlan,
) -> io::Result<(client::SendRequest<Bytes>, tokio::task::JoinHandle<()>)>
where
    T: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (sender, connection) = pair;
    let description = plan.description.clone();
    let handle = tokio::spawn(async move {
        if let Err(err) = connection.await {
            warn!(target = %description, "Upstream HTTP/2 connection ended: {}", err);
        }
    });
    Ok((sender, handle))
}

async fn proxy_http2_stream(
    outbound_sender: &mut client::SendRequest<Bytes>,
    request: Request<RecvStream>,
    respond: &mut server::SendResponse<Bytes>,
    context: Arc<ConnectionContext>,
) -> io::Result<()> {
    let (mut parts, body) = request.into_parts();
    let method = parts.method.clone();
    let path = parts.uri.path().to_string();
    let authority = parts
        .uri
        .authority()
        .map(|auth| auth.to_string())
        .unwrap_or_default();

    ensure_environment_tracestate(&mut parts.headers, context.target_environment_id)?;

    info!(
        target = %context.description,
        branch = ?context.branch_id,
        client = %context.client_addr,
        proxy_port = context.proxy_port,
        service_port = context.service_port,
        "HTTP/2 {} {} (authority {}) forwarded",
        method,
        path,
        authority
    );

    forward_http2_via_sender(outbound_sender, &parts, body, respond).await
}

async fn forward_http2_via_sender(
    sender: &mut client::SendRequest<Bytes>,
    parts: &http::request::Parts,
    body: RecvStream,
    respond: &mut server::SendResponse<Bytes>,
) -> io::Result<()> {
    let outbound_request = build_outbound_request(parts)?;
    let end_of_stream = body.is_end_stream();

    let mut ready_sender = sender.clone().ready().await.map_err(map_h2_err)?;

    let (response_future, outbound_stream) = ready_sender
        .send_request(outbound_request, end_of_stream)
        .map_err(map_h2_err)?;
    *sender = ready_sender;

    let request_forward = async move {
        if end_of_stream {
            Ok(())
        } else {
            forward_request_body(body, outbound_stream).await
        }
    };

    let response_forward = async move {
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
    };

    tokio::try_join!(request_forward, response_forward)?;

    Ok(())
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

async fn forward_request_body(
    mut body: RecvStream,
    mut outbound_stream: SendStream<Bytes>,
) -> io::Result<()> {
    while let Some(chunk_result) = body.data().await {
        let data = chunk_result.map_err(map_h2_err)?;
        let len = data.len();
        outbound_stream.send_data(data, false).map_err(map_h2_err)?;
        if let Err(err) = body.flow_control().release_capacity(len) {
            warn!("Failed to release HTTP/2 request capacity: {}", err);
        }
    }

    if let Some(trailers) = body.trailers().await.map_err(map_h2_err)? {
        outbound_stream
            .send_trailers(trailers)
            .map_err(map_h2_err)?;
    } else {
        outbound_stream
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

    let new_value = if let Some(existing_value) = headers.get(TRACESTATE_HEADER) {
        let existing = existing_value
            .to_str()
            .map(|s| s.to_string())
            .unwrap_or_else(|_| String::from_utf8_lossy(existing_value.as_bytes()).to_string());

        let mut entries: Vec<String> = Vec::new();

        for entry in existing.split(',') {
            let trimmed = entry.trim();
            if trimmed.is_empty() {
                continue;
            }

            if let Some((key, _)) = trimmed.split_once('=') {
                if key.trim().eq_ignore_ascii_case("lapdev-env-id") {
                    continue;
                }
            }

            entries.push(trimmed.to_string());
        }

        entries.push(target_entry);
        entries.join(",")
    } else {
        target_entry
    };

    let header_value = HeaderValue::from_str(&new_value)
        .map_err(|err| io::Error::new(io::ErrorKind::InvalidInput, err))?;
    headers.insert(HeaderName::from_static(TRACESTATE_HEADER), header_value);

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_tracestate_inserted_when_missing() {
        let mut headers = HeaderMap::new();
        let env_id = Uuid::parse_str("aaaaaaaa-bbbb-cccc-dddd-eeeeeeeeeeee").unwrap();

        ensure_environment_tracestate(&mut headers, env_id).unwrap();

        let expected = format!("lapdev-env-id={}", env_id);
        assert_eq!(
            headers
                .get(TRACESTATE_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap(),
            expected
        );
    }

    #[test]
    fn ensure_tracestate_replaces_existing_entry() {
        let mut headers = HeaderMap::new();
        headers.insert(
            HeaderName::from_static(TRACESTATE_HEADER),
            HeaderValue::from_static("foo=bar, lapdev-env-id=old-env ,baz=qux"),
        );

        let env_id = Uuid::parse_str("11111111-2222-3333-4444-555555555555").unwrap();

        ensure_environment_tracestate(&mut headers, env_id).unwrap();

        let expected = format!("foo=bar,baz=qux,lapdev-env-id={}", env_id);
        assert_eq!(
            headers
                .get(TRACESTATE_HEADER)
                .and_then(|value| value.to_str().ok())
                .unwrap(),
            expected
        );
    }
}
