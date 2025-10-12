use anyhow::Result;
use axum::{
    body::Body,
    http::{Response, StatusCode},
    response::IntoResponse,
};
use std::{io, sync::Arc};
use tokio::{
    io::{copy_bidirectional, AsyncWriteExt},
    net::{TcpListener, TcpStream},
};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

use crate::{
    preview_url::{PreviewUrlError, PreviewUrlResolver, PreviewUrlTarget},
    tunnel::TunnelRegistry,
};
use lapdev_db::api::DbApi;
use lapdev_kube_rpc::http_parser;

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
        // Use the shared HTTP parser that handles incremental reading
        let mut buffer = Vec::new();

        match http_parser::parse_complete_http_request(&mut stream, &mut buffer).await {
            Ok((parsed_request, headers_len)) => {
                debug!(
                    "Parsed request: {} {}",
                    parsed_request.method, parsed_request.path
                );

                // Extract Host header using the shared utility
                let host = http_parser::get_host_header(&parsed_request.headers)
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
                    &buffer,
                    headers_len,
                    target.environment_id,
                )?;

                // Start direct TCP proxying
                self.start_tcp_proxy(stream, target, initial_request_data)
                    .await
            }
            Err(e)
                if e.kind() == io::ErrorKind::InvalidData
                    && e.to_string().contains("exceed maximum size") =>
            {
                // Request headers are too large - reject it
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
        mut client_stream: TcpStream,
        target: PreviewUrlTarget,
        initial_request_data: Vec<u8>,
    ) -> Result<(), ProxyError> {
        info!(
            "Starting TCP proxy to {}:{} in cluster {}",
            target.service_name, target.service_port, target.cluster_id
        );

        let tunnel_client = self
            .tunnel_registry
            .get_client(target.cluster_id)
            .await
            .ok_or_else(|| {
                ProxyError::TunnelNotAvailable(format!(
                    "No active tunnel connection for cluster {}",
                    target.cluster_id
                ))
            })?;

        if tunnel_client.is_closed() {
            return Err(ProxyError::TunnelNotAvailable(format!(
                "Tunnel connection for cluster {} is closed",
                target.cluster_id
            )));
        }

        // Build the target host for the service inside the cluster
        let target_host = format!(
            "{}.{}.svc.cluster.local",
            target.service_name, target.namespace
        );

        debug!("Target: {}:{}", target_host, target.service_port);

        let mut tunnel_stream = tunnel_client
            .connect_tcp(target_host.clone(), target.service_port)
            .await
            .map_err(|e| {
                ProxyError::TunnelNotAvailable(format!("Failed to open tunnel connection: {}", e))
            })?;

        if !initial_request_data.is_empty() {
            tunnel_stream
                .write_all(&initial_request_data)
                .await
                .map_err(|e| {
                    ProxyError::NetworkError(format!(
                        "Failed to send initial request through tunnel: {}",
                        e
                    ))
                })?;
        }

        let (bytes_tx, bytes_rx) = copy_bidirectional(&mut client_stream, &mut tunnel_stream)
            .await
            .map_err(|e| ProxyError::NetworkError(format!("Tunnel proxying failed: {}", e)))?;

        info!(
            "Tunnel proxied {} bytes upstream and {} bytes downstream (cluster={})",
            bytes_tx, bytes_rx, target.cluster_id
        );

        if let Err(err) = AsyncWriteExt::shutdown(&mut tunnel_stream).await {
            debug!("Failed to shutdown tunnel stream cleanly: {}", err);
        }

        Ok(())
    }

    /// Add environment ID to the tracestate header in HTTP request using parsed header info
    fn add_environment_id_to_headers(
        &self,
        request_data: &[u8],
        headers_len: usize,
        environment_id: Uuid,
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
    fn test_shared_http_parser_integration() {
        // Test shared HTTP parser to ensure it works as expected
        let request_data = b"GET /path HTTP/1.1\r\nHost: webapp-8080-abc123.example.com\r\nContent-Length: 13\r\n\r\nHello, world!";

        let (parsed_request, body_start) =
            http_parser::parse_http_request_from_buffer(request_data).unwrap();

        assert_eq!(parsed_request.method, "GET");
        assert_eq!(parsed_request.path, "/path");

        // Find Host header using shared utility
        let host_header = http_parser::get_host_header(&parsed_request.headers);
        assert!(host_header.is_some());
        assert_eq!(host_header.unwrap(), "webapp-8080-abc123.example.com");

        // Find Content-Length header using shared utility
        let content_length = http_parser::get_content_length(&parsed_request.headers);
        assert_eq!(content_length, Some(13));

        // Verify body parsing
        let body = &request_data[body_start..];
        assert_eq!(body, b"Hello, world!");
    }
}
