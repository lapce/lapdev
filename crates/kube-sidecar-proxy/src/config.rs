use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::net::SocketAddr;
use uuid::Uuid;

/// Configuration for the sidecar proxy
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProxyConfig {
    /// Address to listen on for incoming requests
    pub listen_addr: SocketAddr,
    
    /// Default target address for proxying requests
    pub default_target: SocketAddr,
    
    /// Kubernetes namespace to operate in
    pub namespace: Option<String>,
    
    /// Pod name for self-identification
    pub pod_name: Option<String>,
    
    /// Lapdev environment ID this proxy belongs to
    pub environment_id: Option<Uuid>,
    
    /// Route configurations
    pub routes: Vec<RouteConfig>,
    
    /// Health check configuration
    pub health_check: HealthCheckConfig,
    
    /// Metrics configuration
    pub metrics: MetricsConfig,
}

/// Individual route configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouteConfig {
    /// Path matcher (supports wildcards)
    pub path: String,
    
    /// Target service to route to
    pub target: RouteTarget,
    
    /// Optional headers to add/modify
    pub headers: HashMap<String, String>,
    
    /// Timeout for this route in milliseconds
    pub timeout_ms: Option<u64>,
    
    /// Whether this route requires authentication
    pub requires_auth: bool,
    
    /// Access level for this route
    pub access_level: AccessLevel,
}

/// Target for routing
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum RouteTarget {
    /// Direct address
    Address(SocketAddr),
    
    /// Kubernetes service
    Service {
        name: String,
        namespace: Option<String>,
        port: u16,
    },
    
    /// Load balance across multiple targets
    LoadBalance(Vec<RouteTarget>),
}

/// Access level for routes (matching Lapdev's preview URL access levels)
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AccessLevel {
    /// Only accessible by the owner with authentication
    Personal,
    /// Accessible by organization members with authentication  
    Shared,
    /// Accessible by anyone without authentication
    Public,
}

/// Health check configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct HealthCheckConfig {
    /// Health check endpoint path
    pub path: String,
    
    /// Interval between health checks in seconds
    pub interval_seconds: u64,
    
    /// Timeout for health checks in milliseconds
    pub timeout_ms: u64,
    
    /// Number of consecutive failures before marking unhealthy
    pub failure_threshold: u32,
}

/// Metrics configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MetricsConfig {
    /// Whether to enable metrics collection
    pub enabled: bool,
    
    /// Metrics endpoint path
    pub path: String,
    
    /// Port to serve metrics on (if different from main port)
    pub port: Option<u16>,
}

impl Default for ProxyConfig {
    fn default() -> Self {
        Self {
            listen_addr: "0.0.0.0:8080".parse().unwrap(),
            default_target: "127.0.0.1:3000".parse().unwrap(),
            namespace: None,
            pod_name: None,
            environment_id: None,
            routes: Vec::new(),
            health_check: HealthCheckConfig::default(),
            metrics: MetricsConfig::default(),
        }
    }
}

impl Default for HealthCheckConfig {
    fn default() -> Self {
        Self {
            path: "/health".to_string(),
            interval_seconds: 30,
            timeout_ms: 5000,
            failure_threshold: 3,
        }
    }
}

impl Default for MetricsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            path: "/metrics".to_string(),
            port: None,
        }
    }
}

/// Kubernetes annotations used for configuration
pub struct ProxyAnnotations;

impl ProxyAnnotations {
    /// Annotation for specifying target service
    pub const TARGET_SERVICE: &'static str = "lapdev.io/proxy-target-service";
    
    /// Annotation for specifying target port
    pub const TARGET_PORT: &'static str = "lapdev.io/proxy-target-port";
    
    /// Annotation for routing rules (JSON)
    pub const ROUTING_RULES: &'static str = "lapdev.io/proxy-routing-rules";
    
    /// Annotation for access level
    pub const ACCESS_LEVEL: &'static str = "lapdev.io/proxy-access-level";
    
    /// Annotation for enabling/disabling authentication
    pub const REQUIRE_AUTH: &'static str = "lapdev.io/proxy-require-auth";
    
    /// Annotation for custom headers (JSON)
    pub const CUSTOM_HEADERS: &'static str = "lapdev.io/proxy-custom-headers";
    
    /// Annotation for timeout in milliseconds
    pub const TIMEOUT_MS: &'static str = "lapdev.io/proxy-timeout-ms";
}