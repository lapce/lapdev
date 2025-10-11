use crate::{
    config::{AccessLevel, ProxyConfig, RouteConfig, RouteTarget},
    discovery::ServiceDiscovery,
    error::{Result, SidecarProxyError},
};
use axum::{
    body::Body,
    extract::{Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri},
    response::{IntoResponse, Response},
    routing::{any, get},
    Router,
};
use hyper_util::{
    client::legacy::{connect::HttpConnector, Client},
    rt::TokioExecutor,
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
use tokio::sync::RwLock;
use tower::ServiceBuilder;
use tower_http::{cors::CorsLayer, timeout::TimeoutLayer, trace::TraceLayer};
use tracing::{debug, error, info, Span};

/// HTTP proxy handler that routes requests based on configuration
pub struct ProxyHandler {
    client: Client<HttpConnector, Body>,
    discovery: Arc<ServiceDiscovery>,
    config: Arc<RwLock<ProxyConfig>>,
}

impl ProxyHandler {
    pub fn new(discovery: Arc<ServiceDiscovery>, config: Arc<RwLock<ProxyConfig>>) -> Self {
        let client = Client::builder(TokioExecutor::new()).build_http::<Body>();

        Self {
            client,
            discovery,
            config,
        }
    }

    /// Create the router with all proxy routes and middleware
    pub fn create_router(self) -> Router {
        let proxy_handler = Arc::new(self);

        Router::new()
            // Health check endpoint (always available)
            .route("/health", get(health_check))
            .route("/ready", get(readiness_check))
            // Metrics endpoint (if enabled)
            .route("/metrics", get(metrics_handler))
            // Catch-all proxy route
            .route("/*path", any(proxy_request))
            .fallback(proxy_request)
            // Add state
            .with_state(proxy_handler)
            // Add middleware
            .layer(
                ServiceBuilder::new()
                    .layer(TraceLayer::new_for_http().on_request(
                        |req: &Request<Body>, _span: &Span| {
                            debug!(
                                method = %req.method(),
                                uri = %req.uri(),
                                "Processing request"
                            );
                        },
                    ))
                    .layer(CorsLayer::permissive())
                    .layer(TimeoutLayer::new(Duration::from_secs(30))),
            )
    }

    /// Proxy a request to the appropriate target
    pub async fn proxy_request(
        &self,
        method: Method,
        uri: Uri,
        headers: HeaderMap,
        body: Body,
    ) -> Result<Response> {
        let path = uri.path();

        // Find matching route
        let route = self.find_matching_route(path).await?;

        // Check if this is a DevboxTunnel route - handle differently
        if let RouteTarget::DevboxTunnel {
            intercept_id,
            session_id,
            target_port,
            auth_token,
        } = &route.target
        {
            return self
                .proxy_request_via_devbox_tunnel(
                    method,
                    uri,
                    headers,
                    body,
                    *intercept_id,
                    *session_id,
                    *target_port,
                    auth_token.clone(),
                )
                .await;
        }

        // Resolve target address (for normal routes)
        let target_addr = self.resolve_target(&route.target).await?;

        // Check authorization if required
        if route.requires_auth {
            self.check_authorization(&headers, &route.access_level)
                .await?;
        }

        // Build target URI
        let target_uri = self.build_target_uri(&target_addr, &uri, &route)?;

        // Prepare headers
        let mut proxy_headers = self.prepare_headers(&headers, &route)?;

        // Create the proxied request
        let mut proxy_req = Request::builder().method(&method).uri(target_uri);

        // Set headers
        for (name, value) in &proxy_headers {
            proxy_req = proxy_req.header(name, value);
        }

        let proxy_req = proxy_req.body(body)?;

        info!(
            method = %method,
            original_path = %uri.path(),
            target = %target_addr,
            "Proxying request"
        );

        // Execute the request with timeout
        let timeout = Duration::from_millis(route.timeout_ms.unwrap_or(30000));

        let response = tokio::time::timeout(timeout, self.client.request(proxy_req))
            .await
            .map_err(|_| SidecarProxyError::Generic(anyhow::anyhow!("Request timeout")))?
            .map_err(|e| SidecarProxyError::Generic(anyhow::anyhow!("HTTP client error: {}", e)))?;

        Ok(response.into_response())
    }

    /// Proxy a request via devbox tunnel for service interception
    async fn proxy_request_via_devbox_tunnel(
        &self,
        _method: Method,
        _uri: Uri,
        _headers: HeaderMap,
        _body: Body,
        intercept_id: uuid::Uuid,
        session_id: uuid::Uuid,
        target_port: u16,
        _auth_token: String,
    ) -> Result<Response> {
        // TODO: Full implementation requires:
        // 1. Access to SidecarProxyManagerRpcClient to request tunnel
        // 2. Bidirectional streaming between incoming request and tunnel data channel
        // 3. Proper framing using ServerTunnelMessage protocol
        // 4. Error handling and cleanup on connection close

        info!(
            "Devbox tunnel proxy requested: intercept_id={}, session_id={}, target_port={}",
            intercept_id, session_id, target_port
        );

        // For now, return error indicating feature is not yet fully implemented
        Err(SidecarProxyError::Generic(anyhow::anyhow!(
            "Devbox tunnel proxying not yet fully implemented. \
             This requires RPC client access and WebSocket tunnel streaming."
        )))
    }

    async fn find_matching_route(&self, path: &str) -> Result<RouteConfig> {
        let config = self.config.read().await;

        // Try to find a matching route
        for route in &config.routes {
            if self.path_matches(&route.path, path) {
                return Ok(route.clone());
            }
        }

        // Default route to default target
        Ok(RouteConfig {
            path: "/*".to_string(),
            target: RouteTarget::Address(config.default_target),
            headers: std::collections::HashMap::new(),
            timeout_ms: None,
            requires_auth: true,
            access_level: AccessLevel::Personal,
        })
    }

    fn path_matches(&self, pattern: &str, path: &str) -> bool {
        // Simple pattern matching - support wildcards
        if pattern.ends_with("/*") {
            let prefix = &pattern[..pattern.len() - 2];
            path.starts_with(prefix)
        } else if pattern == "/*" {
            true
        } else {
            pattern == path
        }
    }

    async fn resolve_target(&self, target: &RouteTarget) -> Result<SocketAddr> {
        match target {
            RouteTarget::Address(addr) => Ok(*addr),
            RouteTarget::Service {
                name,
                namespace,
                port,
            } => {
                let endpoints = self
                    .discovery
                    .resolve_service_target(name, namespace.as_deref(), *port)
                    .await?;

                // Simple round-robin selection (could be enhanced with proper load balancing)
                endpoints
                    .first()
                    .cloned()
                    .ok_or_else(|| SidecarProxyError::TargetNotFound {
                        service_name: name.clone(),
                    })
            }
            RouteTarget::LoadBalance(targets) => {
                // Simple implementation - pick first available target that's directly an address
                for target in targets {
                    match target {
                        RouteTarget::Address(addr) => return Ok(*addr),
                        RouteTarget::Service {
                            name,
                            namespace,
                            port,
                        } => {
                            if let Ok(endpoints) = self
                                .discovery
                                .resolve_service_target(name, namespace.as_deref(), *port)
                                .await
                            {
                                if let Some(addr) = endpoints.first() {
                                    return Ok(*addr);
                                }
                            }
                        }
                        _ => continue, // Skip nested LoadBalance and DevboxTunnel to avoid recursion
                    }
                }
                Err(SidecarProxyError::ServiceDiscovery(
                    "No healthy targets available".to_string(),
                ))
            }
            RouteTarget::DevboxTunnel { .. } => {
                // DevboxTunnel routing is handled separately via tunnel establishment
                // This shouldn't be called for DevboxTunnel targets
                Err(SidecarProxyError::Generic(anyhow::anyhow!(
                    "DevboxTunnel routing requires tunnel establishment, not direct resolution"
                )))
            }
        }
    }

    async fn check_authorization(
        &self,
        headers: &HeaderMap,
        access_level: &AccessLevel,
    ) -> Result<()> {
        match access_level {
            AccessLevel::Public => {
                // No authorization required
                Ok(())
            }
            AccessLevel::Personal | AccessLevel::Shared => {
                // Check for valid authentication headers
                // This would integrate with Lapdev's existing auth system
                if headers.contains_key("authorization") || headers.contains_key("cookie") {
                    // TODO: Validate token/session against Lapdev auth system
                    debug!("Authorization check passed (stub implementation)");
                    Ok(())
                } else {
                    Err(SidecarProxyError::Authorization(
                        "Authentication required".to_string(),
                    ))
                }
            }
        }
    }

    fn build_target_uri(
        &self,
        target_addr: &SocketAddr,
        original_uri: &Uri,
        _route: &RouteConfig,
    ) -> Result<Uri> {
        let scheme = "http"; // Could be enhanced to support HTTPS
        let path_and_query = original_uri
            .path_and_query()
            .map(|pq| pq.as_str())
            .unwrap_or("/");

        let target_uri = format!("{}://{}{}", scheme, target_addr, path_and_query);

        target_uri
            .parse()
            .map_err(|e| SidecarProxyError::Generic(anyhow::anyhow!("Invalid target URI: {}", e)))
    }

    fn prepare_headers(&self, original: &HeaderMap, route: &RouteConfig) -> Result<HeaderMap> {
        let mut headers = original.clone();

        // Remove hop-by-hop headers
        headers.remove("connection");
        headers.remove("proxy-connection");
        headers.remove("te");
        headers.remove("trailers");
        headers.remove("upgrade");

        // Add custom headers from route configuration
        for (name, value) in &route.headers {
            let header_name = HeaderName::from_bytes(name.as_bytes()).map_err(|e| {
                SidecarProxyError::Generic(anyhow::anyhow!("Invalid header name: {}", e))
            })?;
            let header_value = HeaderValue::from_str(value).map_err(|e| {
                SidecarProxyError::Generic(anyhow::anyhow!("Invalid header value: {}", e))
            })?;
            headers.insert(header_name, header_value);
        }

        // Add X-Forwarded headers
        if let Some(host) = headers.get("host") {
            headers.insert("x-forwarded-host", host.clone());
        }
        headers.insert("x-forwarded-proto", HeaderValue::from_static("http"));

        Ok(headers)
    }
}

// Handler functions

async fn proxy_request(
    State(handler): State<Arc<ProxyHandler>>,
    req: Request,
) -> impl IntoResponse {
    let (parts, body) = req.into_parts();

    match handler
        .proxy_request(parts.method, parts.uri, parts.headers, body)
        .await
    {
        Ok(response) => response,
        Err(e) => {
            error!("Proxy error: {}", e);
            match e {
                SidecarProxyError::TargetNotFound { .. } => {
                    (StatusCode::BAD_GATEWAY, format!("Target not found: {}", e)).into_response()
                }
                SidecarProxyError::Authorization(_) => {
                    (StatusCode::UNAUTHORIZED, "Unauthorized").into_response()
                }
                _ => (StatusCode::INTERNAL_SERVER_ERROR, "Internal server error").into_response(),
            }
        }
    }
}

async fn health_check() -> impl IntoResponse {
    (StatusCode::OK, "OK")
}

async fn readiness_check(State(handler): State<Arc<ProxyHandler>>) -> impl IntoResponse {
    // Check if we can reach the default target
    let config = handler.config.read().await;
    let target = config.default_target;

    // Simple TCP connection check
    match tokio::net::TcpStream::connect(target).await {
        Ok(_) => (StatusCode::OK, "Ready"),
        Err(_) => (StatusCode::SERVICE_UNAVAILABLE, "Not ready"),
    }
}

async fn metrics_handler() -> impl IntoResponse {
    // TODO: Implement Prometheus metrics
    // For now, return basic metrics
    let metrics = r#"
# HELP http_requests_total Total number of HTTP requests
# TYPE http_requests_total counter
http_requests_total 0

# HELP http_request_duration_seconds HTTP request duration in seconds
# TYPE http_request_duration_seconds histogram
http_request_duration_seconds_bucket{le="0.1"} 0
http_request_duration_seconds_bucket{le="0.5"} 0
http_request_duration_seconds_bucket{le="1.0"} 0
http_request_duration_seconds_bucket{le="+Inf"} 0
http_request_duration_seconds_sum 0
http_request_duration_seconds_count 0
"#;

    (StatusCode::OK, metrics)
}
