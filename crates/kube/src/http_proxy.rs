use anyhow::Result;
use axum::{
    http::StatusCode,
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
use lapdev_common::{kube::PreviewUrlAccessLevel, LAPDEV_AUTH_TOKEN_COOKIE};
use lapdev_db::api::DbApi;
use lapdev_kube_rpc::http_parser;
use pasetors::{
    claims::ClaimsValidationRules, keys::SymmetricKey, local, token::UntrustedToken, version4::V4,
};

const PAGE_NOT_AUTHORISED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../api/pages/not_authorised.html"
));
const PAGE_NOT_FOUND: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../api/pages/not_found.html"
));
const PAGE_NOT_FORWARDED: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../api/pages/not_forwarded.html"
));
const PAGE_NOT_RUNNING: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../api/pages/not_running.html"
));
const PAGE_GENERIC_ERROR: &str = include_str!(concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../api/pages/generic_error.html"
));

pub struct PreviewUrlProxy {
    url_resolver: PreviewUrlResolver,
    tunnel_registry: Arc<TunnelRegistry>,
    db: DbApi,
    auth_token_key: Arc<SymmetricKey<V4>>,
}

impl PreviewUrlProxy {
    pub async fn new(db: DbApi, tunnel_registry: Arc<TunnelRegistry>) -> Self {
        let auth_token_key = Arc::new(db.load_api_auth_token_key().await);
        let url_resolver = PreviewUrlResolver::new(db.clone());

        Self {
            url_resolver,
            tunnel_registry,
            db,
            auth_token_key,
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
                let target = match self.resolve_preview_url_target(&subdomain).await {
                    Ok(target) => target,
                    Err(err) => {
                        self.respond_with_proxy_error(&mut stream, err).await?;
                        return Ok(());
                    }
                };

                // Enforce access controls based on preview URL configuration
                if let Err(err) = self.authorize_request(&parsed_request, &target).await {
                    self.respond_with_proxy_error(&mut stream, err).await?;
                    return Ok(());
                }

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
                self.send_error_page(
                    &mut stream,
                    StatusCode::PAYLOAD_TOO_LARGE,
                    PAGE_GENERIC_ERROR,
                )
                .await
            }
            Err(_) => {
                self.send_error_page(&mut stream, StatusCode::BAD_REQUEST, PAGE_GENERIC_ERROR)
                    .await
            }
        }
    }

    /// Send error response to client
    async fn send_error_page(
        &self,
        stream: &mut TcpStream,
        status: StatusCode,
        body: &'static str,
    ) -> Result<(), ProxyError> {
        let status_line = format!(
            "HTTP/1.1 {} {}\r\n",
            status.as_u16(),
            status.canonical_reason().unwrap_or("Unknown")
        );
        stream
            .write_all(status_line.as_bytes())
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to write status line: {e}")))?;

        let headers = format!(
            "Content-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );

        stream
            .write_all(headers.as_bytes())
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to write headers: {e}")))?;

        stream
            .write_all(body.as_bytes())
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to write body: {e}")))?;

        stream
            .flush()
            .await
            .map_err(|e| ProxyError::Internal(format!("Failed to flush response: {e}")))?;

        Ok(())
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

    async fn respond_with_proxy_error(
        &self,
        stream: &mut TcpStream,
        error: ProxyError,
    ) -> Result<(), ProxyError> {
        let (status, page) = match &error {
            ProxyError::Forbidden(_) => (StatusCode::FORBIDDEN, PAGE_NOT_AUTHORISED),
            ProxyError::NotFound(_) => (StatusCode::NOT_FOUND, PAGE_NOT_FOUND),
            ProxyError::TunnelNotAvailable(_) => {
                (StatusCode::SERVICE_UNAVAILABLE, PAGE_NOT_FORWARDED)
            }
            ProxyError::Timeout(_) => (StatusCode::GATEWAY_TIMEOUT, PAGE_NOT_RUNNING),
            ProxyError::InvalidUrl(_) => (StatusCode::BAD_REQUEST, PAGE_GENERIC_ERROR),
            ProxyError::NetworkError(_) => (StatusCode::BAD_GATEWAY, PAGE_GENERIC_ERROR),
            ProxyError::Internal(_) => (StatusCode::INTERNAL_SERVER_ERROR, PAGE_GENERIC_ERROR),
        };

        warn!("Responding with error page: {:?} ({})", status, error);

        self.send_error_page(stream, status, page).await
    }

    /// Ensure the caller is authorized to access the resolved preview target
    async fn authorize_request(
        &self,
        request: &http_parser::ParsedHttpRequest,
        target: &PreviewUrlTarget,
    ) -> Result<(), ProxyError> {
        match target.access_level {
            PreviewUrlAccessLevel::Public => Ok(()),
            PreviewUrlAccessLevel::Organization => {
                let token = self
                    .extract_cookie_value(&request.headers, LAPDEV_AUTH_TOKEN_COOKIE)
                    .ok_or_else(|| ProxyError::Forbidden("Authentication required".to_string()))?;

                let user_id = self.user_id_from_token(&token)?;

                self.ensure_org_membership(user_id, target.organization_id)
                    .await?;

                debug!(
                    "Authorized organization member {} for preview {}",
                    user_id, target.preview_url_id
                );

                Ok(())
            }
        }
    }

    fn extract_cookie_value(
        &self,
        headers: &[(String, String)],
        cookie_name: &str,
    ) -> Option<String> {
        let prefix = format!("{cookie_name}=");

        headers
            .iter()
            .filter(|(name, _)| name.eq_ignore_ascii_case("cookie"))
            .flat_map(|(_, value)| value.split(';'))
            .map(|cookie| cookie.trim())
            .find_map(|cookie| cookie.strip_prefix(&prefix).map(|value| value.to_string()))
    }

    fn user_id_from_token(&self, token: &str) -> Result<Uuid, ProxyError> {
        let untrusted =
            UntrustedToken::try_from(token).map_err(|_| ProxyError::Forbidden("Invalid authentication token".to_string()))?;

        let trusted = local::decrypt(
            self.auth_token_key.as_ref(),
            &untrusted,
            &ClaimsValidationRules::new(),
            None,
            None,
        )
            .map_err(|_| ProxyError::Forbidden("Invalid authentication token".to_string()))?;

        let claims = trusted
            .payload_claims()
            .ok_or_else(|| ProxyError::Forbidden("Invalid authentication token".to_string()))?;

        let user_id_value = claims
            .get_claim("user_id")
            .ok_or_else(|| ProxyError::Forbidden("Invalid authentication token".to_string()))?;

        let user_id: String = serde_json::from_value(user_id_value.clone())
            .map_err(|_| ProxyError::Forbidden("Invalid authentication token".to_string()))?;

        Uuid::parse_str(&user_id)
            .map_err(|_| ProxyError::Forbidden("Invalid authentication token".to_string()))
    }

    async fn ensure_org_membership(
        &self,
        user_id: Uuid,
        organization_id: Uuid,
    ) -> Result<(), ProxyError> {
        self.db
            .get_organization_member(user_id, organization_id)
            .await
            .map(|_| ())
            .map_err(|err| {
                if err
                    .to_string()
                    .contains("no organization member found")
                {
                    ProxyError::Forbidden("Organization membership required".to_string())
                } else {
                    error!(
                        "Failed to verify organization membership for user {} in organization {}: {}",
                        user_id, organization_id, err
                    );
                    ProxyError::Internal(
                        "Failed to verify organization membership".to_string(),
                    )
                }
            })
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
