use anyhow::Result;
use axum::{
    body::Body,
    http::{HeaderMap, HeaderName, HeaderValue, Method, Request, Response, StatusCode, Uri},
    response::IntoResponse,
};
use std::sync::Arc;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    preview_url::{PreviewUrlError, PreviewUrlResolver, PreviewUrlTarget},
    tunnel::{TunnelRegistry, TunnelResponse},
};
use lapdev_db::api::DbApi;

pub struct PreviewUrlProxy {
    url_resolver: PreviewUrlResolver,
    tunnel_registry: Arc<TunnelRegistry>,
}

impl PreviewUrlProxy {
    pub fn new(db: DbApi, tunnel_registry: Arc<TunnelRegistry>) -> Self {
        Self {
            url_resolver: PreviewUrlResolver::new(db),
            tunnel_registry,
        }
    }

    /// Start the standalone TCP server for preview URL proxy
    pub async fn start_tcp_server(self: Arc<Self>, bind_addr: &str) -> Result<(), ProxyError> {
        let listener = TcpListener::bind(bind_addr)
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to bind to {}: {}", bind_addr, e)))?;

        info!("PreviewUrlProxy TCP server listening on {}", bind_addr);

        loop {
            match listener.accept().await {
                Ok((stream, client_addr)) => {
                    debug!("New client connection from: {}", client_addr);
                    let proxy = Arc::clone(&self);

                    // Spawn a new task for each client connection
                    tokio::spawn(async move {
                        if let Err(e) = proxy.handle_client_connection(stream).await {
                            error!(
                                "Error handling client connection from {}: {}",
                                client_addr, e
                            );
                        }
                    });
                }
                Err(e) => {
                    error!("Failed to accept client connection: {}", e);
                    // Continue accepting other connections
                    continue;
                }
            }
        }
    }

    /// Handle a single client TCP connection with direct TCP stream proxying
    async fn handle_client_connection(&self, mut stream: TcpStream) -> Result<(), ProxyError> {
        // Use a 64KB buffer - should handle virtually all HTTP requests without looping
        let mut buffer = vec![0u8; 64 * 1024];
        let bytes_read = stream
            .read(&mut buffer)
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to read from stream: {e}")))?;

        if bytes_read == 0 {
            return Err(ProxyError::Internal(
                "Connection closed unexpectedly".to_string(),
            ));
        }

        // Parse HTTP request using httparse
        let mut headers_buf = [httparse::EMPTY_HEADER; 64];
        let mut req = httparse::Request::new(&mut headers_buf);

        match req.parse(&buffer[..bytes_read]) {
            Ok(httparse::Status::Complete(headers_len)) => {
                let method = req
                    .method
                    .ok_or_else(|| ProxyError::Internal("Missing HTTP method".to_string()))?;
                let method = Method::from_bytes(method.as_bytes())
                    .map_err(|_| ProxyError::Internal("Invalid HTTP method".to_string()))?;

                let path = req
                    .path
                    .ok_or_else(|| ProxyError::Internal("Missing request path".to_string()))?;
                debug!("Parsed request: {} {}", method, path);

                // Extract Host header
                let mut host_header = None;
                for header in req.headers {
                    if header.name.to_lowercase() == "host" {
                        if let Ok(host_str) = std::str::from_utf8(header.value) {
                            host_header = Some(host_str.to_string());
                            break;
                        }
                    }
                }

                let host = host_header
                    .ok_or_else(|| ProxyError::InvalidUrl("Missing Host header".to_string()))?;

                let subdomain = host
                    .split('.')
                    .next()
                    .ok_or_else(|| ProxyError::InvalidUrl("Invalid host format".to_string()))?
                    .to_string();

                debug!("Extracted subdomain: {} from host: {}", subdomain, host);

                // Resolve preview URL target
                let target = self.resolve_preview_url_target(&subdomain).await?;

                info!(
                    "Resolved target: service={}:{} in cluster={}",
                    target.service_name, target.service_port, target.cluster_id
                );

                // Modify the initial request data to add environment ID to tracestate header
                let initial_request_data = self.add_environment_id_to_headers(
                    &buffer[..bytes_read],
                    headers_len,
                    target.environment_id,
                )?;

                // Start direct TCP proxying
                self.start_tcp_proxy(stream, target, initial_request_data)
                    .await
            }
            Ok(httparse::Status::Partial) => {
                // Request is larger than 64KB - reject it
                self.send_error_response(&mut stream, 413, "Request Entity Too Large")
                    .await
            }
            Err(_) => {
                self.send_error_response(&mut stream, 400, "Bad Request")
                    .await
            }
        }
    }

    /// Send HTTP response back to client
    async fn send_http_response(
        &self,
        stream: &mut TcpStream,
        response: Response<Body>,
    ) -> Result<(), ProxyError> {
        let (parts, body) = response.into_parts();

        // Write status line
        let status_line = format!(
            "HTTP/1.1 {} {}\r\n",
            parts.status.as_u16(),
            parts.status.canonical_reason().unwrap_or("Unknown")
        );
        stream
            .write_all(status_line.as_bytes())
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to write status line: {e}")))?;

        // Write headers
        for (name, value) in parts.headers.iter() {
            let header_line = format!(
                "{}: {}\r\n",
                name.as_str(),
                value
                    .to_str()
                    .map_err(|e| ProxyError::Internal(format!("Invalid header value: {e}")))?
            );
            stream
                .write_all(header_line.as_bytes())
                .await
                .map_err(|e| ProxyError::Internal(format!("Failed to write header: {e}")))?;
        }

        // End headers
        stream
            .write_all(b"\r\n")
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to write header separator: {e}")))?;

        // Write body
        let body_bytes = axum::body::to_bytes(body, usize::MAX)
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to read response body: {e}")))?;

        stream
            .write_all(&body_bytes)
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to write response body: {e}")))?;

        stream
            .flush()
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to flush stream: {e}")))?;

        Ok(())
    }

    /// Send error response to client
    async fn send_error_response(
        &self,
        stream: &mut TcpStream,
        status_code: u16,
        reason: &str,
    ) -> Result<(), ProxyError> {
        use axum::body::Body;

        let response_body = format!("<h1>{} {}</h1>", status_code, reason);

        let status = StatusCode::from_u16(status_code)
            .map_err(|_| ProxyError::Internal(format!("Invalid status code: {}", status_code)))?;

        let response = Response::builder()
            .status(status)
            .header("Content-Type", "text/html")
            .header("Content-Length", response_body.len())
            .header("Connection", "close")
            .body(Body::from(response_body))
            .map_err(|e| ProxyError::Internal(format!("Failed to build error response: {}", e)))?;

        // Reuse the existing send_http_response method
        self.send_http_response(stream, response).await
    }

    /// Handle preview URL requests
    pub async fn handle_preview_url(
        &self,
        subdomain: &str,
        method: Method,
        headers: HeaderMap,
        body: Vec<u8>,
        path_and_query: &str,
    ) -> Result<Response<Body>, ProxyError> {
        info!("Handling preview URL request: {}", subdomain);

        // 1. Parse subdomain
        let url_info = PreviewUrlResolver::parse_preview_url(subdomain)
            .map_err(|e| ProxyError::InvalidUrl(e.to_string()))?;

        debug!("Parsed URL info: {:?}", url_info);

        // 2. Resolve preview URL target
        let target = self
            .url_resolver
            .resolve_preview_url(url_info)
            .await
            .map_err(|e| match e {
                PreviewUrlError::EnvironmentNotFound => {
                    ProxyError::NotFound("Environment not found".to_string())
                }
                PreviewUrlError::ServiceNotFound => {
                    ProxyError::NotFound("Service not found".to_string())
                }
                PreviewUrlError::PreviewUrlNotConfigured => {
                    ProxyError::NotFound("Preview URL not configured".to_string())
                }
                PreviewUrlError::AccessDenied => ProxyError::Forbidden("Access denied".to_string()),
                _ => ProxyError::Internal(e.to_string()),
            })?;

        info!(
            "Resolved target: service={}:{} in cluster={}",
            target.service_name, target.service_port, target.cluster_id
        );

        debug!(
            "Using tunnel for cluster: {:?} (assuming KubeManager available)",
            target.cluster_id
        );

        // 3. Forward HTTP request through tunnel
        let response = self
            .forward_http_request(
                target.cluster_id,
                &target,
                method,
                headers,
                body,
                path_and_query,
            )
            .await?;

        // 4. Update access timestamp
        if let Err(e) = self
            .url_resolver
            .update_preview_url_access(target.preview_url_id)
            .await
        {
            warn!("Failed to update preview URL access timestamp: {}", e);
        }

        Ok(response)
    }

    /// Forward HTTP request through the TCP tunnel
    async fn forward_http_request(
        &self,
        cluster_id: Uuid,
        target: &PreviewUrlTarget,
        method: Method,
        headers: HeaderMap,
        body: Vec<u8>,
        path_and_query: &str,
    ) -> Result<Response<Body>, ProxyError> {
        info!(
            "Forwarding {} request to {}:{} via TCP tunnel",
            method, target.service_name, target.service_port
        );

        // Build the target host for the service inside the cluster
        let target_host = format!(
            "{}.{}.svc.cluster.local",
            target.service_name, target.namespace
        );

        debug!("Target: {}:{}", target_host, target.service_port);

        // Implement TCP-over-WebSocket tunneling
        let response = self
            .forward_via_websocket_tunnel(
                cluster_id,
                target,
                &method,
                headers,
                body,
                path_and_query,
                &target_host,
            )
            .await?;

        info!(
            "TCP tunnel placeholder response for {} request to {}:{}",
            method, target_host, target.service_port
        );

        Ok(response)
    }

    /// Forward HTTP request via WebSocket tunnel
    /// Uses the existing established websocket connection from TunnelRegistry
    async fn forward_via_websocket_tunnel(
        &self,
        cluster_id: Uuid,
        target: &PreviewUrlTarget,
        method: &Method,
        headers: HeaderMap,
        body: Vec<u8>,
        path_and_query: &str,
        target_host: &str,
    ) -> Result<Response<Body>, ProxyError> {
        use lapdev_kube_rpc::ServerTunnelMessage;
        use std::time::Duration;

        info!(
            "Using established WebSocket tunnel for cluster: {}",
            cluster_id
        );

        // Generate unique tunnel ID for this HTTP request
        let tunnel_id = format!("http_{}", uuid::Uuid::new_v4());

        // Build HTTP request using http library, then serialize headers for streaming
        let http_headers_bytes = self.serialize_http_request_headers(
            method.clone(),
            headers.clone(),
            body.len(),
            path_and_query,
            target_host,
        )?;

        // Create data channel to receive HTTP response data
        let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        self.tunnel_registry
            .register_data_channel(tunnel_id.clone(), data_tx)
            .await;

        // 1. Send OpenConnection message and wait for response
        let open_msg = ServerTunnelMessage::OpenConnection {
            tunnel_id: tunnel_id.clone(),
            target_host: target_host.to_string(),
            target_port: target.service_port,
            protocol_hint: Some("http".to_string()),
        };

        let connection_response = self
            .tunnel_registry
            .send_tunnel_message_with_response(cluster_id, open_msg, Duration::from_secs(10))
            .await
            .map_err(|e| {
                ProxyError::TunnelNotAvailable(format!("Failed to open connection: {}", e))
            })?;

        // Handle connection response
        match connection_response {
            TunnelResponse::ConnectionOpened { .. } => {
                debug!("Connection opened successfully for tunnel: {}", tunnel_id);
            }
            TunnelResponse::ConnectionFailed { error, .. } => {
                self.tunnel_registry.cleanup_tunnel(&tunnel_id).await;
                return Err(ProxyError::TunnelNotAvailable(format!(
                    "Connection failed: {}",
                    error
                )));
            }
            _ => {
                self.tunnel_registry.cleanup_tunnel(&tunnel_id).await;
                return Err(ProxyError::Internal("Unexpected response type".to_string()));
            }
        }

        // 2. Send HTTP request through tunnel (always streamed)
        // Send headers first
        let header_msg = ServerTunnelMessage::Data {
            tunnel_id: tunnel_id.clone(),
            payload: http_headers_bytes,
            sequence_num: Some(1),
        };

        self.tunnel_registry
            .send_tunnel_message(cluster_id, header_msg)
            .await
            .map_err(|e| {
                ProxyError::TunnelNotAvailable(format!("Failed to send request headers: {}", e))
            })?;

        // Send body in chunks (if there is a body)
        if !body.is_empty() {
            const CHUNK_SIZE: usize = 32 * 1024; // 32KB chunks
            let mut sequence_num = 2;

            for chunk in body.chunks(CHUNK_SIZE) {
                let chunk_msg = ServerTunnelMessage::Data {
                    tunnel_id: tunnel_id.clone(),
                    payload: chunk.to_vec(),
                    sequence_num: Some(sequence_num),
                };

                self.tunnel_registry
                    .send_tunnel_message(cluster_id, chunk_msg)
                    .await
                    .map_err(|e| {
                        ProxyError::TunnelNotAvailable(format!(
                            "Failed to send request chunk: {}",
                            e
                        ))
                    })?;

                sequence_num += 1;

                // Small delay to prevent overwhelming the tunnel
                tokio::time::sleep(Duration::from_millis(1)).await;
            }

            debug!("Streamed request body in {} chunks", sequence_num - 2);
        } else {
            debug!("Sent request headers only (no body)");
        }

        // 3. Handle HTTP response with streaming support
        let response = self
            .collect_streaming_response(
                data_rx,
                Duration::from_secs(30), // HTTP request timeout
            )
            .await?;

        // Clean up
        self.cleanup_tunnel_connection(cluster_id, &tunnel_id).await;

        Ok(response)
    }

    /// Collect streaming HTTP response data
    async fn collect_streaming_response(
        &self,
        mut data_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
        timeout: std::time::Duration,
    ) -> Result<Response<Body>, ProxyError> {
        let mut response_data = Vec::new();
        let mut headers_parsed = false;
        let mut response_builder = None;
        let mut content_length = None;
        let mut is_chunked = false;
        let mut body_data = Vec::new();
        let mut headers_end_pos = None;

        let start_time = std::time::Instant::now();

        while start_time.elapsed() < timeout {
            match tokio::time::timeout(std::time::Duration::from_millis(500), data_rx.recv()).await
            {
                Ok(Some(data)) => {
                    response_data.extend_from_slice(&data);

                    // First, try to parse headers if not done yet
                    if !headers_parsed {
                        if let Some(end_pos) =
                            response_data.windows(4).position(|w| w == b"\r\n\r\n")
                        {
                            headers_end_pos = Some(end_pos + 4);

                            // Parse HTTP response using http library approach
                            let (parsed_response, _body_start_pos) =
                                self.parse_http_response_headers(&response_data[..end_pos + 4])?;

                            // Extract parsed information
                            let (parts, _) = parsed_response.into_parts();

                            // Check for Content-Length and Transfer-Encoding
                            if let Some(cl_header) = parts.headers.get("content-length") {
                                content_length = cl_header
                                    .to_str()
                                    .ok()
                                    .and_then(|s| s.parse::<usize>().ok());
                            }

                            if let Some(te_header) = parts.headers.get("transfer-encoding") {
                                if te_header
                                    .to_str()
                                    .map_or(false, |s| s.to_lowercase().contains("chunked"))
                                {
                                    is_chunked = true;
                                }
                            }

                            // Rebuild response builder with parsed parts
                            let mut builder = Response::builder()
                                .status(parts.status)
                                .version(parts.version);

                            // Add all headers
                            for (name, value) in parts.headers.iter() {
                                builder = builder.header(name, value);
                            }

                            response_builder = Some(builder);
                            headers_parsed = true;

                            // Extract body data that we've already received
                            if response_data.len() > end_pos + 4 {
                                body_data.extend_from_slice(&response_data[end_pos + 4..]);
                            }

                            debug!(
                                "Parsed headers - Content-Length: {:?}, Chunked: {}",
                                content_length, is_chunked
                            );
                        }
                    } else {
                        // Headers already parsed, collect body data
                        if let Some(end_pos) = headers_end_pos {
                            let new_body_start = response_data.len() - data.len();
                            if new_body_start >= end_pos {
                                body_data.extend_from_slice(&data);
                            } else {
                                // Handle case where data spans across headers/body boundary
                                let body_part_start = end_pos.saturating_sub(new_body_start);
                                if body_part_start < data.len() {
                                    body_data.extend_from_slice(&data[body_part_start..]);
                                }
                            }
                        }
                    }

                    // Check if we have complete response
                    if headers_parsed {
                        if let Some(expected_length) = content_length {
                            if body_data.len() >= expected_length {
                                // Complete response with Content-Length
                                let final_body = body_data[..expected_length].to_vec();
                                let response = response_builder
                                    .unwrap()
                                    .body(Body::from(final_body))
                                    .map_err(|e| {
                                        ProxyError::Internal(format!(
                                            "Failed to build response: {}",
                                            e
                                        ))
                                    })?;
                                return Ok(response);
                            }
                        } else if is_chunked {
                            // Handle chunked encoding - decode what we have so far
                            if let Some(decoded_body) = self.try_decode_chunked_body(&body_data)? {
                                // We have a complete chunked response
                                let response = response_builder
                                    .unwrap()
                                    .header("content-length", decoded_body.len())
                                    .body(Body::from(decoded_body))
                                    .map_err(|e| {
                                        ProxyError::Internal(format!(
                                            "Failed to build chunked response: {}",
                                            e
                                        ))
                                    })?;
                                return Ok(response);
                            }
                        }
                        // For responses without Content-Length or chunked encoding, we need to wait for connection close
                    }
                }
                Ok(None) => {
                    // Channel closed, assume response is complete
                    break;
                }
                Err(_) => {
                    // Timeout on individual recv, continue waiting
                    continue;
                }
            }
        }

        // Handle response completion or timeout
        if headers_parsed {
            if let Some(builder) = response_builder {
                // Connection closed or timeout, use whatever body data we have
                let response = builder.body(Body::from(body_data)).map_err(|e| {
                    ProxyError::Internal(format!("Failed to build response: {}", e))
                })?;
                return Ok(response);
            }
        }

        if response_data.is_empty() {
            Err(ProxyError::Timeout("No response data received".to_string()))
        } else {
            Err(ProxyError::Internal("Incomplete HTTP response".to_string()))
        }
    }

    /// Parse HTTP response headers using http library
    fn parse_http_response_headers(
        &self,
        data: &[u8],
    ) -> Result<(Response<()>, usize), ProxyError> {
        // Find the end of headers
        let headers_end = data
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .ok_or_else(|| ProxyError::Internal("Headers not complete".to_string()))?;

        // Convert to string for parsing
        let headers_str = std::str::from_utf8(&data[..headers_end])
            .map_err(|e| ProxyError::Internal(format!("Invalid UTF-8 in headers: {}", e)))?;

        // Parse the status line
        let mut lines = headers_str.lines();
        let status_line = lines
            .next()
            .ok_or_else(|| ProxyError::Internal("Missing status line".to_string()))?;

        // Extract status code from status line (e.g., "HTTP/1.1 200 OK")
        let status_parts: Vec<&str> = status_line.split_whitespace().collect();
        if status_parts.len() < 2 {
            return Err(ProxyError::Internal(
                "Invalid status line format".to_string(),
            ));
        }

        let status_code = status_parts[1]
            .parse::<u16>()
            .map_err(|_| ProxyError::Internal("Invalid status code".to_string()))?;

        // Build response using http library
        let mut response_builder = Response::builder().status(
            StatusCode::from_u16(status_code)
                .map_err(|_| ProxyError::Internal("Invalid status code value".to_string()))?,
        );

        // Parse and add headers
        for line in lines {
            if line.trim().is_empty() {
                continue;
            }
            if let Some((name, value)) = line.split_once(':') {
                let name = name.trim();
                let value = value.trim();

                // Validate header name and value using http library types
                let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                    ProxyError::Internal(format!("Invalid header name '{}': {}", name, e))
                })?;
                let header_value = HeaderValue::from_str(value).map_err(|e| {
                    ProxyError::Internal(format!("Invalid header value '{}': {}", value, e))
                })?;

                response_builder = response_builder.header(header_name, header_value);
            }
        }

        let response = response_builder
            .body(())
            .map_err(|e| ProxyError::Internal(format!("Failed to build response: {}", e)))?;

        Ok((response, headers_end + 4))
    }

    /// Try to decode chunked body if complete
    fn try_decode_chunked_body(&self, body_data: &[u8]) -> Result<Option<Vec<u8>>, ProxyError> {
        let mut pos = 0;
        let mut decoded_body = Vec::new();

        while pos < body_data.len() {
            // Find chunk size line (ends with \r\n)
            if let Some(size_end) = body_data[pos..].windows(2).position(|w| w == b"\r\n") {
                let size_line = std::str::from_utf8(&body_data[pos..pos + size_end])
                    .map_err(|_| ProxyError::Internal("Invalid chunk size".to_string()))?;

                // Parse chunk size (hex)
                let chunk_size = usize::from_str_radix(size_line.trim(), 16)
                    .map_err(|_| ProxyError::Internal("Invalid chunk size format".to_string()))?;

                pos += size_end + 2; // Skip size line and \r\n

                if chunk_size == 0 {
                    // End of chunks, look for final \r\n
                    if pos + 1 < body_data.len() && &body_data[pos..pos + 2] == b"\r\n" {
                        // Complete chunked response
                        return Ok(Some(decoded_body));
                    } else {
                        // Incomplete final chunk
                        return Ok(None);
                    }
                }

                // Check if we have the complete chunk data + \r\n
                if pos + chunk_size + 2 <= body_data.len() {
                    decoded_body.extend_from_slice(&body_data[pos..pos + chunk_size]);
                    pos += chunk_size + 2; // Skip chunk data and trailing \r\n
                } else {
                    // Incomplete chunk data
                    return Ok(None);
                }
            } else {
                // No complete chunk size line yet
                return Ok(None);
            }
        }

        // If we get here, we're still expecting more chunks
        Ok(None)
    }

    /// Build HTTP request headers using http library and serialize for streaming
    fn serialize_http_request_headers(
        &self,
        method: Method,
        mut headers: HeaderMap,
        body_length: usize,
        path_and_query: &str,
        host: &str,
    ) -> Result<Vec<u8>, ProxyError> {
        // Ensure Host header is set
        headers.insert(
            "Host",
            HeaderValue::from_str(host)
                .map_err(|e| ProxyError::Internal(format!("Invalid host header: {}", e)))?,
        );

        // Set Content-Length if there's a body
        if body_length > 0 {
            headers.insert(
                "Content-Length",
                HeaderValue::from_str(&body_length.to_string())
                    .map_err(|e| ProxyError::Internal(format!("Invalid content-length: {}", e)))?,
            );
        }

        // Build the request using http library (but only for validation and structure)
        let uri = Uri::from_maybe_shared(format!("http://{}{}", host, path_and_query))
            .map_err(|e| ProxyError::Internal(format!("Invalid URI: {}", e)))?;

        let _request = Request::builder()
            .method(method.clone())
            .uri(&uri)
            .body(())
            .map_err(|e| ProxyError::Internal(format!("Failed to build request: {}", e)))?;

        // Manually serialize the request line and headers for streaming
        // We do this because we need raw bytes for tunnel, not the http::Request structure
        let mut request_bytes = Vec::new();

        // Request line
        request_bytes
            .extend_from_slice(format!("{} {} HTTP/1.1\r\n", method, path_and_query).as_bytes());

        // Headers (http library ensures proper formatting)
        for (name, value) in &headers {
            request_bytes.extend_from_slice(
                format!(
                    "{}: {}\r\n",
                    name.as_str(),
                    value.to_str().map_err(|e| ProxyError::Internal(format!(
                        "Invalid header value: {}",
                        e
                    )))?
                )
                .as_bytes(),
            );
        }

        // End headers
        request_bytes.extend_from_slice(b"\r\n");

        Ok(request_bytes)
    }

    /// Helper method to clean up tunnel connection
    async fn cleanup_tunnel_connection(&self, cluster_id: Uuid, tunnel_id: &str) {
        // Send close message
        let close_msg = lapdev_kube_rpc::ServerTunnelMessage::CloseConnection {
            tunnel_id: tunnel_id.to_string(),
            reason: lapdev_kube_rpc::CloseReason::ClientRequest,
        };

        let _ = self
            .tunnel_registry
            .send_tunnel_message(cluster_id, close_msg)
            .await;

        // Clean up registry resources
        self.tunnel_registry.cleanup_tunnel(tunnel_id).await;
    }

    /// Resolve preview URL target from subdomain
    async fn resolve_preview_url_target(
        &self,
        subdomain: &str,
    ) -> Result<PreviewUrlTarget, ProxyError> {
        // Parse subdomain
        let url_info = PreviewUrlResolver::parse_preview_url(subdomain)
            .map_err(|e| ProxyError::InvalidUrl(e.to_string()))?;

        debug!("Parsed URL info: {:?}", url_info);

        // Resolve preview URL target
        let target = self
            .url_resolver
            .resolve_preview_url(url_info)
            .await
            .map_err(|e| match e {
                PreviewUrlError::EnvironmentNotFound => {
                    ProxyError::NotFound("Environment not found".to_string())
                }
                PreviewUrlError::ServiceNotFound => {
                    ProxyError::NotFound("Service not found".to_string())
                }
                PreviewUrlError::PreviewUrlNotConfigured => {
                    ProxyError::NotFound("Preview URL not configured".to_string())
                }
                PreviewUrlError::AccessDenied => ProxyError::Forbidden("Access denied".to_string()),
                _ => ProxyError::Internal(e.to_string()),
            })?;

        // Update access timestamp
        if let Err(e) = self
            .url_resolver
            .update_preview_url_access(target.preview_url_id)
            .await
        {
            warn!("Failed to update preview URL access timestamp: {}", e);
        }

        Ok(target)
    }

    /// Start direct TCP proxying between client and target service
    async fn start_tcp_proxy(
        &self,
        client_stream: TcpStream,
        target: PreviewUrlTarget,
        initial_request_data: Vec<u8>,
    ) -> Result<(), ProxyError> {
        use lapdev_kube_rpc::ServerTunnelMessage;
        use std::time::Duration;

        info!(
            "Starting TCP proxy to {}:{} in cluster {}",
            target.service_name, target.service_port, target.cluster_id
        );

        // Generate unique tunnel ID for this TCP connection
        let tunnel_id = format!("tcp_{}", uuid::Uuid::new_v4());

        // Build the target host for the service inside the cluster
        let target_host = format!(
            "{}.{}.svc.cluster.local",
            target.service_name, target.namespace
        );

        debug!("Target: {}:{}", target_host, target.service_port);

        // Create data channel to receive data from the tunnel
        let (data_tx, data_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
        self.tunnel_registry
            .register_data_channel(tunnel_id.clone(), data_tx)
            .await;

        // 1. Send OpenConnection message and wait for response
        let open_msg = ServerTunnelMessage::OpenConnection {
            tunnel_id: tunnel_id.clone(),
            target_host: target_host.clone(),
            target_port: target.service_port,
            protocol_hint: Some("tcp".to_string()),
        };

        let connection_response = self
            .tunnel_registry
            .send_tunnel_message_with_response(target.cluster_id, open_msg, Duration::from_secs(10))
            .await
            .map_err(|e| {
                ProxyError::TunnelNotAvailable(format!("Failed to open connection: {}", e))
            })?;

        // Handle connection response
        match connection_response {
            TunnelResponse::ConnectionOpened { .. } => {
                info!(
                    "TCP tunnel connection opened successfully for tunnel: {}",
                    tunnel_id
                );
            }
            TunnelResponse::ConnectionFailed { error, .. } => {
                self.tunnel_registry.cleanup_tunnel(&tunnel_id).await;
                return Err(ProxyError::TunnelNotAvailable(format!(
                    "Connection failed: {}",
                    error
                )));
            }
            _ => {
                self.tunnel_registry.cleanup_tunnel(&tunnel_id).await;
                return Err(ProxyError::Internal("Unexpected response type".to_string()));
            }
        }

        // 2. Send the initial request data through the tunnel (with environment ID in tracestate)
        if !initial_request_data.is_empty() {
            let data_msg = ServerTunnelMessage::Data {
                tunnel_id: tunnel_id.clone(),
                payload: initial_request_data,
                sequence_num: Some(1),
            };

            self.tunnel_registry
                .send_tunnel_message(target.cluster_id, data_msg)
                .await
                .map_err(|e| {
                    ProxyError::TunnelNotAvailable(format!("Failed to send initial data: {}", e))
                })?;

            debug!("Sent initial request data with environment ID in tracestate through tunnel");
        }

        // 3. Start bidirectional TCP proxying
        let proxy_result = self
            .run_bidirectional_proxy(client_stream, target.cluster_id, tunnel_id.clone(), data_rx)
            .await;

        // Clean up tunnel connection
        self.cleanup_tunnel_connection(target.cluster_id, &tunnel_id)
            .await;

        proxy_result
    }

    /// Run bidirectional TCP proxy between client stream and tunnel
    async fn run_bidirectional_proxy(
        &self,
        client_stream: TcpStream,
        cluster_id: Uuid,
        tunnel_id: String,
        mut tunnel_data_rx: tokio::sync::mpsc::UnboundedReceiver<Vec<u8>>,
    ) -> Result<(), ProxyError> {
        use lapdev_kube_rpc::ServerTunnelMessage;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let (client_read, client_write) = client_stream.into_split();
        let mut client_read = client_read;
        let mut client_write = client_write;

        let tunnel_registry = Arc::clone(&self.tunnel_registry);
        let tunnel_id_clone = tunnel_id.clone();

        // Task 1: Read from client and send to tunnel
        let client_to_tunnel = {
            let tunnel_registry = tunnel_registry.clone();
            let tunnel_id = tunnel_id_clone.clone();
            tokio::spawn(async move {
                let mut buffer = vec![0u8; 8192];
                let mut sequence_num = 2u32; // Starting from 2 since initial request was 1

                loop {
                    match client_read.read(&mut buffer).await {
                        Ok(0) => {
                            debug!("Client connection closed (read EOF)");
                            break;
                        }
                        Ok(n) => {
                            let data = buffer[..n].to_vec();

                            let data_msg = ServerTunnelMessage::Data {
                                tunnel_id: tunnel_id.clone(),
                                payload: data,
                                sequence_num: Some(sequence_num),
                            };

                            if let Err(e) = tunnel_registry
                                .send_tunnel_message(cluster_id, data_msg)
                                .await
                            {
                                error!("Failed to send client data to tunnel: {}", e);
                                break;
                            }

                            sequence_num = sequence_num.wrapping_add(1);
                        }
                        Err(e) => {
                            debug!("Client read error: {}", e);
                            break;
                        }
                    }
                }

                debug!("Client-to-tunnel task completed");
            })
        };

        // Task 2: Read from tunnel and send to client
        let tunnel_to_client = tokio::spawn(async move {
            while let Some(data) = tunnel_data_rx.recv().await {
                if let Err(e) = client_write.write_all(&data).await {
                    debug!("Client write error: {}", e);
                    break;
                }

                if let Err(e) = client_write.flush().await {
                    debug!("Client flush error: {}", e);
                    break;
                }
            }

            debug!("Tunnel-to-client task completed");
        });

        // Wait for either task to complete (indicating connection closure)
        let result = tokio::select! {
            _ = client_to_tunnel => {
                debug!("Client-to-tunnel task finished first");
                Ok(())
            }
            _ = tunnel_to_client => {
                debug!("Tunnel-to-client task finished first");
                Ok(())
            }
        };

        info!("TCP proxy session completed for tunnel: {}", tunnel_id);
        result
    }

    /// Add environment ID to the tracestate header in HTTP request using parsed header info
    fn add_environment_id_to_headers(
        &self,
        request_data: &[u8],
        headers_len: usize,
        environment_id: uuid::Uuid,
    ) -> Result<Vec<u8>, ProxyError> {
        // Split headers and body based on headers_len from httparse
        let headers_part = &request_data[..headers_len];
        let body_part = &request_data[headers_len..];

        // Convert headers to string for modification
        let headers_str = std::str::from_utf8(headers_part)
            .map_err(|e| ProxyError::Internal(format!("Invalid UTF-8 in headers: {}", e)))?;

        let environment_tracestate = format!("lapdev-env-id={}", environment_id);
        let mut modified_headers = String::new();
        let mut tracestate_added = false;

        for line in headers_str.lines() {
            if line.to_lowercase().starts_with("tracestate:") {
                // Existing tracestate header - append our environment ID
                let existing_value = line[11..].trim(); // Skip "tracestate:"
                if existing_value.is_empty() {
                    modified_headers
                        .push_str(&format!("tracestate: {}\r\n", environment_tracestate));
                } else {
                    modified_headers.push_str(&format!(
                        "tracestate: {},{}\r\n",
                        existing_value, environment_tracestate
                    ));
                }
                tracestate_added = true;
            } else if line.is_empty() && !tracestate_added {
                // Add tracestate before the empty line that separates headers from body
                modified_headers.push_str(&format!("tracestate: {}\r\n", environment_tracestate));
                modified_headers.push_str("\r\n");
                tracestate_added = true;
            } else {
                modified_headers.push_str(line);
                modified_headers.push_str("\r\n");
            }
        }

        // If we didn't add tracestate yet (no empty line found), add it at the end
        if !tracestate_added {
            modified_headers.push_str(&format!("tracestate: {}\r\n", environment_tracestate));
        }

        // Reconstruct the complete request
        let mut modified_request = Vec::new();
        modified_request.extend_from_slice(modified_headers.as_bytes());
        modified_request.extend_from_slice(body_part);

        debug!(
            "Added environment ID {} to tracestate header",
            environment_id
        );

        Ok(modified_request)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("Invalid URL: {0}")]
    InvalidUrl(String),
    #[error("Not found: {0}")]
    NotFound(String),
    #[error("Forbidden: {0}")]
    Forbidden(String),
    #[error("Tunnel not available: {0}")]
    TunnelNotAvailable(String),
    #[error("Timeout: {0}")]
    Timeout(String),
    #[error("Network error: {0}")]
    NetworkError(String),
    #[error("Internal error: {0}")]
    Internal(String),
}

impl IntoResponse for ProxyError {
    fn into_response(self) -> axum::response::Response {
        let (status_code, error_message) = match self {
            ProxyError::InvalidUrl(msg) => (StatusCode::BAD_REQUEST, msg),
            ProxyError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
            ProxyError::Forbidden(msg) => (StatusCode::FORBIDDEN, msg),
            ProxyError::TunnelNotAvailable(msg) => (StatusCode::SERVICE_UNAVAILABLE, msg),
            ProxyError::Timeout(msg) => (StatusCode::GATEWAY_TIMEOUT, msg),
            ProxyError::NetworkError(msg) => (StatusCode::BAD_GATEWAY, msg),
            ProxyError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
        };

        (status_code, error_message).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tunnel::TunnelRegistry;
    use lapdev_db::api::DbApi;
    use std::sync::Arc;
    use tokio::sync::RwLock;
    use uuid::Uuid;

    // Mock test for basic URL parsing flow
    #[tokio::test]
    async fn test_preview_url_parsing() {
        // This would require a proper database setup for full testing
        // For now, we can test the URL parsing logic
        use crate::preview_url::PreviewUrlResolver;

        let result = PreviewUrlResolver::parse_preview_url("webapp-8080-abc123");
        assert!(result.is_ok());

        let info = result.unwrap();
        assert_eq!(info.service_name, "webapp");
        assert_eq!(info.port, 8080);
        assert_eq!(info.environment_hash, "abc123");
    }

    #[test]
    fn test_proxy_error_types() {
        let invalid_url_error = ProxyError::InvalidUrl("test".to_string());
        let not_found_error = ProxyError::NotFound("test".to_string());
        let forbidden_error = ProxyError::Forbidden("test".to_string());
        let tunnel_error = ProxyError::TunnelNotAvailable("test".to_string());
        let timeout_error = ProxyError::Timeout("test".to_string());
        let network_error = ProxyError::NetworkError("test".to_string());
        let internal_error = ProxyError::Internal("test".to_string());

        // Test that all error variants can be created and displayed
        assert!(invalid_url_error.to_string().contains("Invalid URL"));
        assert!(not_found_error.to_string().contains("Not found"));
        assert!(forbidden_error.to_string().contains("Forbidden"));
        assert!(tunnel_error.to_string().contains("Tunnel not available"));
        assert!(timeout_error.to_string().contains("Timeout"));
        assert!(network_error.to_string().contains("Network error"));
        assert!(internal_error.to_string().contains("Internal error"));
    }

    #[test]
    fn test_httparse_integration() {
        // Test httparse parsing directly to ensure it works as expected
        let request_data = b"GET /path HTTP/1.1\r\nHost: webapp-8080-abc123.example.com\r\nContent-Length: 13\r\n\r\nHello, world!";

        let mut headers = [httparse::EMPTY_HEADER; 16];
        let mut req = httparse::Request::new(&mut headers);

        let result = req.parse(request_data).unwrap();
        assert!(matches!(result, httparse::Status::Complete(_)));

        assert_eq!(req.method, Some("GET"));
        assert_eq!(req.path, Some("/path"));

        // Find Host header
        let host_header = req.headers.iter().find(|h| h.name == "Host");
        assert!(host_header.is_some());
        assert_eq!(
            host_header.unwrap().value,
            b"webapp-8080-abc123.example.com"
        );

        // Find Content-Length header
        let content_length_header = req.headers.iter().find(|h| h.name == "Content-Length");
        assert!(content_length_header.is_some());
        assert_eq!(content_length_header.unwrap().value, b"13");
    }
}
