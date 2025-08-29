# Preview URL Proxy Implementation Plan

## 🏗️ **Final Architecture**

```
Browser → KubeController → WebSocket TCP Tunnel → KubeManager → Target Service
            ↑                    ↑                     ↑              ↑
      Preview URL Handler   TCP-over-WebSocket    TCP Client    Any Protocol
      • Parse subdomain     • Connection mux      • Service     • HTTP/WS/gRPC
      • DB lookup          • Raw TCP data         discovery     • Database
      • Authentication     • Bidirectional       • Direct      • SSH, etc.
      • Access control     • Multiple conns       connect
```

## 📋 **Implementation Plan**

### ✅ **Phase 1: Architecture & Design (COMPLETED)**
- [x] Analyze current sidecar proxy implementation and RPC patterns
- [x] Design preview URL proxy architecture and traffic flow  
- [x] Design TCP-over-WebSocket tunneling protocol

### 🚧 **Phase 2: Core Infrastructure (IN PROGRESS)**

#### **2.1 Control Plane Setup**
- [ ] **Define control plane RPC methods for tunnel management**

**Extend existing `KubeClusterRpc` for tunnel management:**

```rust
#[tarpc::service]
pub trait KubeClusterRpc {
    async fn report_cluster_info(cluster_info: KubeClusterInfo) -> Result<(), String>;
    
    // NEW: Tunnel lifecycle management
    async fn register_tunnel(
        tunnel_endpoint: String,
        supported_protocols: Vec<String>
    ) -> Result<TunnelRegistrationResponse, String>;
    
    async fn tunnel_heartbeat() -> Result<(), String>;
    
    async fn report_tunnel_metrics(
        active_connections: u32,
        bytes_transferred: u64,
        connection_count: u64,
        connection_errors: u64,
    ) -> Result<(), String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelRegistrationResponse {
    pub tunnel_id: String,
    pub websocket_endpoint: String, // e.g., "wss://controller.lap.dev/tunnel/abc123"
    pub heartbeat_interval_seconds: u32,
}
```

**Extend `KubeManagerRpc` with tunnel support:**

```rust
#[tarpc::service]
pub trait KubeManagerRpc {
    // ... existing 13 methods from current implementation ...
    
    // NEW: Preview URL tunnel methods
    async fn establish_data_plane_tunnel(
        controller_endpoint: String,
        auth_token: String,
    ) -> Result<TunnelEstablishmentResponse, String>;
    
    async fn get_tunnel_status() -> Result<TunnelStatus, String>;
    
    async fn close_tunnel_connection(tunnel_id: String) -> Result<(), String>;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelEstablishmentResponse {
    pub success: bool,
    pub tunnel_id: String,
    pub websocket_endpoint: String,
    pub error_message: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelStatus {
    pub tunnel_id: Option<String>,
    pub is_connected: bool,
    pub connected_at: Option<chrono::DateTime<chrono::Utc>>,
    pub last_heartbeat: Option<chrono::DateTime<chrono::Utc>>,
    pub active_connections: u32,
    pub total_connections: u64,
    pub bytes_transferred: u64,
}
```

**KubeController Components:**

```rust
// Tunnel registry in KubeController (in-memory only)
pub struct TunnelRegistry {
    active_tunnels: Arc<RwLock<HashMap<Uuid, TunnelConnection>>>, // cluster_id -> tunnel
    tunnel_metrics: Arc<RwLock<HashMap<Uuid, TunnelMetrics>>>,
}

#[derive(Debug, Clone)]
pub struct TunnelConnection {
    pub cluster_id: Uuid,
    pub websocket: Arc<WebSocketSender>,
    pub endpoint: String,
    pub connected_at: chrono::DateTime<chrono::Utc>,
    pub last_heartbeat: chrono::DateTime<chrono::Utc>,
}

#[derive(Debug, Clone)]
pub struct TunnelMetrics {
    pub active_connections: u32,
    pub total_connections: u64,
    pub bytes_transferred: u64,
    pub connection_errors: u64,
}

impl TunnelRegistry {
    pub async fn register_tunnel(&self, cluster_id: Uuid, websocket: WebSocketSender) -> Result<()> {
        let mut tunnels = self.active_tunnels.write().await;
        tunnels.insert(cluster_id, TunnelConnection {
            cluster_id,
            websocket: Arc::new(websocket),
            endpoint: format!("/tunnel/{}", cluster_id),
            connected_at: chrono::Utc::now(),
            last_heartbeat: chrono::Utc::now(),
        });
        tracing::info!("Registered tunnel for cluster: {}", cluster_id);
        Ok(())
    }
    
    pub async fn get_tunnel(&self, cluster_id: Uuid) -> Option<Arc<WebSocketSender>> {
        let tunnels = self.active_tunnels.read().await;
        tunnels.get(&cluster_id).map(|conn| conn.websocket.clone())
    }
}
```

**Additional Data Structures:**

```rust
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TunnelInfo {
    pub tunnel_id: String,
    pub websocket_endpoint: String,
    pub supported_protocols: Vec<String>, // ["tcp", "http", "websocket"]
    pub max_concurrent_connections: u32,
}
```

**Implementation Steps:**

1. **Extend existing RPC traits** in `lapdev-kube-rpc/src/lib.rs`
2. **Implement tunnel methods** in existing KubeClusterRpc server (KubeController)
3. **Extend KubeManagerRpc** with tunnel methods in KubeManager
4. **Add TunnelRegistry** to KubeController for managing active tunnels (in-memory)
5. **Modify KubeManager** to establish data plane WebSocket connection
6. **Add tunnel heartbeat mechanism** for connection health monitoring

#### **2.2 Data Plane Implementation** 
- [ ] **Implement TCP tunnel server in KubeController**
  - WebSocket tunnel endpoint (`/tunnel`)
  - TCP proxy server for preview URLs
  - Connection multiplexing over single WebSocket
  
- [ ] **Implement WebSocket TCP tunnel client in KubeManager**
  - Persistent WebSocket connection to KubeController
  - TCP connection management to target services
  - Bidirectional data forwarding

- [ ] **Add TCP connection multiplexing over single WebSocket**
  - Tunnel ID management and routing
  - Connection lifecycle (open/close/error handling)
  - Raw TCP data serialization over WebSocket

### 🔗 **Phase 3: Application Integration**

#### **3.1 Preview URL Handling**
- [ ] **Add preview URL parsing and database lookups**
  
**Database Integration (using existing entities):**

```rust
// Preview URL resolution using existing database schema
pub struct PreviewUrlResolver {
    db: DbApi,
}

impl PreviewUrlResolver {
    // Parse subdomain format: service-port-hash.app.lap.dev
    pub fn parse_preview_url(subdomain: &str) -> Result<PreviewUrlInfo, ParseError> {
        // Expected format: "webapp-8080-abc123" 
        let parts: Vec<&str> = subdomain.split('-').collect();
        if parts.len() < 3 {
            return Err(ParseError::InvalidFormat);
        }
        
        let service_name = parts[0..parts.len()-2].join("-"); // Handle multi-part service names
        let port: u16 = parts[parts.len()-2].parse()
            .map_err(|_| ParseError::InvalidPort)?;
        let env_hash = parts[parts.len()-1];
        
        Ok(PreviewUrlInfo {
            service_name,
            port,
            environment_hash: env_hash.to_string(),
        })
    }
    
    // Database lookup using existing entities
    pub async fn resolve_preview_url(&self, info: PreviewUrlInfo) -> Result<PreviewUrlTarget, ResolveError> {
        // 1. Find environment by hash (stored in kube_environment.name or metadata)
        let environment = self.db.find_environment_by_hash(&info.environment_hash).await?
            .ok_or(ResolveError::EnvironmentNotFound)?;
            
        // 2. Find matching service in kube_environment_service table
        let service = self.db.find_environment_service(
            environment.id,
            &info.service_name
        ).await?
            .ok_or(ResolveError::ServiceNotFound)?;
        
        // 3. Find preview URL configuration in kube_environment_preview_url table
        let preview_url = self.db.find_preview_url_by_service_port(
            environment.id,
            service.id,
            info.port as i32
        ).await?
            .ok_or(ResolveError::PreviewUrlNotConfigured)?;
        
        // 4. Validate access level and permissions
        self.validate_access_level(&preview_url).await?;
        
        Ok(PreviewUrlTarget {
            cluster_id: environment.cluster_id,
            namespace: environment.namespace,
            service_name: service.name,
            service_port: info.port,
            access_level: preview_url.access_level,
            protocol: preview_url.protocol,
        })
    }
}

#[derive(Debug, Clone)]
pub struct PreviewUrlInfo {
    pub service_name: String,
    pub port: u16,
    pub environment_hash: String,
}

#[derive(Debug, Clone)]
pub struct PreviewUrlTarget {
    pub cluster_id: Uuid,
    pub namespace: String,
    pub service_name: String,
    pub service_port: u16,
    pub access_level: String, // "personal", "shared", "public"
    pub protocol: String,     // "http", "tcp", "websocket"
}
```

**Required database methods to add to DbApi:**

```rust
impl DbApi {
    // Find environment by hash - may need to add hash field or use existing field
    async fn find_environment_by_hash(&self, hash: &str) -> Result<Option<kube_environment::Model>, DbError>;
    
    // Find service by environment and name
    async fn find_environment_service(&self, env_id: Uuid, service_name: &str) -> Result<Option<kube_environment_service::Model>, DbError>;
    
    // Find preview URL configuration
    async fn find_preview_url_by_service_port(&self, env_id: Uuid, service_id: Uuid, port: i32) -> Result<Option<kube_environment_preview_url::Model>, DbError>;
    
    // Update last accessed timestamp
    async fn update_preview_url_access(&self, preview_url_id: Uuid) -> Result<(), DbError>;
}
```

#### **3.2 End-to-End Flow**
- [ ] **Connect control plane and data plane for end-to-end flow**
  - Preview URL endpoint in KubeController
  - Route resolution and tunnel selection
  - Complete request/response flow

### 🔐 **Phase 4: Security & Access Control**
- [ ] **Implement authentication and access control**

**4.1 Preview URL Access Control:**

```rust
pub struct PreviewUrlAccessValidator {
    db: DbApi,
    session_manager: SessionManager,
}

impl PreviewUrlAccessValidator {
    pub async fn validate_access(
        &self,
        request: &HttpRequest,
        preview_url: &kube_environment_preview_url::Model,
        environment: &kube_environment::Model,
    ) -> Result<AccessResult, AccessError> {
        match preview_url.access_level.as_str() {
            "public" => {
                // Public access - no authentication required
                Ok(AccessResult::Allowed)
            },
            "shared" => {
                // Shared access - require valid lapdev session
                let session = self.extract_session_token(request)?;
                let user = self.session_manager.validate_session(session).await?;
                
                // Check if user has access to the organization
                if user.organization_id == environment.organization_id {
                    self.update_access_log(preview_url.id, user.id).await?;
                    Ok(AccessResult::Allowed)
                } else {
                    Ok(AccessResult::Forbidden("Organization access required".to_string()))
                }
            },
            "personal" => {
                // Personal access - require owner or admin
                let session = self.extract_session_token(request)?;
                let user = self.session_manager.validate_session(session).await?;
                
                if user.id == environment.user_id || 
                   user.id == preview_url.created_by ||
                   self.is_organization_admin(user.id, environment.organization_id).await? {
                    self.update_access_log(preview_url.id, user.id).await?;
                    Ok(AccessResult::Allowed)
                } else {
                    Ok(AccessResult::Forbidden("Owner access required".to_string()))
                }
            },
            _ => Ok(AccessResult::Forbidden("Invalid access level".to_string())),
        }
    }
    
    fn extract_session_token(&self, request: &HttpRequest) -> Result<String, AccessError> {
        // Extract from Authorization header or lapdev_session cookie
        request.headers()
            .get("Authorization")
            .and_then(|h| h.to_str().ok())
            .and_then(|h| h.strip_prefix("Bearer "))
            .map(|token| token.to_string())
            .or_else(|| {
                request.cookie("lapdev_session")
                    .map(|cookie| cookie.value().to_string())
            })
            .ok_or(AccessError::NoCredentials)
    }
}

#[derive(Debug)]
pub enum AccessResult {
    Allowed,
    Forbidden(String),
    Unauthorized(String),
}
```

**4.2 Cluster-to-Controller Authentication:**

```rust
pub struct ClusterAuthentication {
    cluster_tokens: Arc<RwLock<HashMap<Uuid, ClusterAuth>>>, // cluster_id -> auth info
    db: DbApi,
}

#[derive(Debug, Clone)]
pub struct ClusterAuth {
    pub cluster_id: Uuid,
    pub token_hash: String,
    pub expires_at: chrono::DateTime<chrono::Utc>,
    pub capabilities: Vec<String>,
}

impl ClusterAuthentication {
    pub async fn authenticate_cluster(&self, cluster_id: Uuid, token: &str) -> Result<ClusterAuth, AuthError> {
        // 1. Verify cluster exists in database
        let cluster = self.db.get_kube_cluster(cluster_id).await?
            .ok_or(AuthError::ClusterNotFound)?;
            
        // 2. Get cluster token from kube_cluster_token table
        let cluster_token = self.db.get_active_cluster_token(cluster_id).await?
            .ok_or(AuthError::InvalidToken)?;
            
        // 3. Verify token hash
        if !self.verify_token_hash(token, &cluster_token.token_hash) {
            return Err(AuthError::InvalidToken);
        }
        
        // 4. Check token expiration
        if cluster_token.expires_at.map_or(false, |exp| exp < chrono::Utc::now()) {
            return Err(AuthError::TokenExpired);
        }
        
        // 5. Store authenticated session
        let auth = ClusterAuth {
            cluster_id,
            token_hash: cluster_token.token_hash.clone(),
            expires_at: cluster_token.expires_at.unwrap_or_else(|| chrono::Utc::now() + chrono::Duration::hours(24)),
            capabilities: vec!["tunnel".to_string(), "tcp".to_string(), "http".to_string()],
        };
        
        let mut tokens = self.cluster_tokens.write().await;
        tokens.insert(cluster_id, auth.clone());
        
        Ok(auth)
    }
    
    fn verify_token_hash(&self, token: &str, stored_hash: &str) -> bool {
        // Use same hashing as used in kube_cluster_token creation
        use sha2::{Sha256, Digest};
        let mut hasher = Sha256::new();
        hasher.update(token.as_bytes());
        let computed_hash = format!("{:x}", hasher.finalize());
        computed_hash == stored_hash
    }
}
```

**4.3 TLS and Network Security:**

```rust
// WebSocket TLS configuration for KubeManager
pub struct TunnelClientConfig {
    pub controller_endpoint: String,           // wss://controller.lap.dev
    pub ca_cert_path: Option<String>,         // Custom CA if needed
    pub client_cert_path: Option<String>,     // mTLS client cert
    pub client_key_path: Option<String>,      // mTLS client key
    pub verify_hostname: bool,                // TLS hostname verification
    pub cluster_id: Uuid,
    pub auth_token: String,                   // From KUBE_CLUSTER_TOKEN_HEADER env var
}

impl TunnelClientConfig {
    pub fn from_env() -> Result<Self, ConfigError> {
        Ok(Self {
            controller_endpoint: std::env::var("LAPDEV_CONTROLLER_ENDPOINT")
                .unwrap_or_else(|_| "wss://controller.lap.dev".to_string()),
            ca_cert_path: std::env::var("LAPDEV_CA_CERT_PATH").ok(),
            client_cert_path: std::env::var("LAPDEV_CLIENT_CERT_PATH").ok(),
            client_key_path: std::env::var("LAPDEV_CLIENT_KEY_PATH").ok(),
            verify_hostname: std::env::var("LAPDEV_VERIFY_HOSTNAME")
                .map(|v| v.parse().unwrap_or(true))
                .unwrap_or(true),
            cluster_id: std::env::var("LAPDEV_CLUSTER_ID")
                .map_err(|_| ConfigError::MissingClusterId)?
                .parse()
                .map_err(|_| ConfigError::InvalidClusterId)?,
            auth_token: std::env::var("KUBE_CLUSTER_TOKEN_HEADER")
                .map_err(|_| ConfigError::MissingAuthToken)?,
        })
    }
}
```

**4.4 Rate Limiting and DDoS Protection:**

```rust
pub struct TunnelRateLimiter {
    connection_limits: Arc<RwLock<HashMap<Uuid, ConnectionQuota>>>, // cluster_id -> quota
    request_limiter: governor::RateLimiter<governor::state::NotKeyed, 
                                          governor::state::InMemoryState, 
                                          governor::clock::DefaultClock>,
}

#[derive(Debug, Clone)]
pub struct ConnectionQuota {
    pub max_concurrent_connections: u32,
    pub current_connections: u32,
    pub max_requests_per_minute: u32,
    pub requests_this_minute: u32,
    pub last_reset: chrono::DateTime<chrono::Utc>,
}

impl TunnelRateLimiter {
    pub async fn check_connection_limit(&self, cluster_id: Uuid) -> Result<(), RateLimitError> {
        let mut limits = self.connection_limits.write().await;
        let quota = limits.entry(cluster_id).or_insert(ConnectionQuota {
            max_concurrent_connections: 100, // Default limit
            current_connections: 0,
            max_requests_per_minute: 1000,
            requests_this_minute: 0,
            last_reset: chrono::Utc::now(),
        });
        
        if quota.current_connections >= quota.max_concurrent_connections {
            return Err(RateLimitError::TooManyConnections);
        }
        
        quota.current_connections += 1;
        Ok(())
    }
}
```

### 🔧 **Phase 5: Production Readiness**

#### **5.1 Error Handling**
- [ ] **Implement error handling and connection cleanup**
  - TCP connection timeouts and retries
  - WebSocket reconnection logic
  - Graceful tunnel cleanup on errors

#### **5.2 Observability**
- [ ] **Add monitoring and metrics for tunnel performance**
  - Connection count and throughput metrics
  - Tunnel latency and error rates
  - Preview URL access patterns

#### **5.3 Infrastructure**
- [ ] **Set up wildcard DNS and TLS for *.app.lap.dev**
  - DNS configuration for preview URL subdomains
  - TLS certificate management (Let's Encrypt)
  - Load balancer configuration

### 🧪 **Phase 6: Testing & Validation**
- [ ] **Test end-to-end preview URL functionality**
  - HTTP request/response flows
  - WebSocket upgrade support
  - Multiple concurrent connections
  - Error scenarios and recovery

## 🔍 **Key Technical Details**

### **TCP Tunneling Protocol:**
```rust
#[derive(Serialize, Deserialize, Debug, Clone)]
#[serde(tag = "type")]
pub enum TunnelMessage {
    // Connection lifecycle
    OpenConnection { 
        tunnel_id: String, 
        target_host: String, 
        target_port: u16,
        protocol_hint: Option<String>, // "http", "websocket", "tcp"
    },
    ConnectionOpened { 
        tunnel_id: String,
        local_addr: String, // KubeManager's local connection address
    },
    ConnectionFailed { 
        tunnel_id: String, 
        error: String,
        error_code: TunnelErrorCode,
    },
    CloseConnection { 
        tunnel_id: String,
        reason: CloseReason,
    },
    ConnectionClosed { 
        tunnel_id: String,
        bytes_transferred: u64,
    },
    
    // Data transfer
    Data { 
        tunnel_id: String, 
        payload: Vec<u8>,
        sequence_num: Option<u32>, // For ordered delivery if needed
    },
    
    // Control messages
    Ping { timestamp: u64 },
    Pong { timestamp: u64 },
    
    // Tunnel management
    TunnelStats {
        active_connections: u32,
        total_connections: u64,
        bytes_sent: u64,
        bytes_received: u64,
        connection_errors: u64,
    },
    
    // Authentication
    Authenticate {
        cluster_id: Uuid,
        auth_token: String,
        tunnel_capabilities: Vec<String>, // ["tcp", "http", "websocket"]
    },
    AuthenticationResult {
        success: bool,
        session_id: Option<String>,
        error_message: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum TunnelErrorCode {
    ConnectionRefused,
    Timeout,
    NetworkUnreachable,
    PermissionDenied,
    InternalError,
}

#[derive(Serialize, Deserialize, Debug, Clone)]
pub enum CloseReason {
    ClientRequest,
    ServerRequest,
    Timeout,
    Error,
    Shutdown,
}

// WebSocket frame structure for binary messages
pub struct TunnelFrame {
    pub message: TunnelMessage,
    pub timestamp: chrono::DateTime<chrono::Utc>,
    pub message_id: Uuid, // For request/response correlation
}

impl TunnelFrame {
    pub fn serialize(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }
    
    pub fn deserialize(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}
```

**WebSocket Connection Protocol:**

1. **Connection Establishment:**
   ```
   KubeManager → WebSocket Connect → KubeController
   KubeManager → Authenticate{cluster_id, token, capabilities}
   KubeController → AuthenticationResult{success, session_id}
   ```

2. **Tunnel Request Flow:**
   ```
   Browser → HTTP Request → KubeController
   KubeController → OpenConnection{tunnel_id, target_host, target_port}
   KubeManager → ConnectionOpened{tunnel_id, local_addr}
   KubeController → Data{tunnel_id, http_request_payload}
   KubeManager → Data{tunnel_id, http_response_payload}
   ```

3. **Connection Management:**
   ```
   Periodic: KubeManager → Ping{timestamp}
   Response: KubeController → Pong{timestamp}
   Stats: KubeManager → TunnelStats{...} (every 30s)
   ```

4. **Error Handling:**
   ```
   On TCP Error: KubeManager → ConnectionFailed{tunnel_id, error, error_code}
   On Close: KubeManager → ConnectionClosed{tunnel_id, bytes_transferred}
   ```

### **Preview URL Format:**
```
https://webapp-8080-xyz123.app.lap.dev/api/users
       ↑      ↑    ↑
   service  port  env_hash
```

### **Data Flow:**
1. Browser → `webapp-8080-xyz123.app.lap.dev`
2. KubeController parses subdomain, validates access
3. Opens TCP tunnel to KubeManager: `webapp.namespace.svc:8080`
4. KubeManager forwards to target service
5. Bidirectional TCP data over WebSocket tunnel

### **Implementation Architecture:**

#### **KubeController Components:**
```rust
// TCP Proxy Server
struct PreviewUrlProxy {
    tunnel_manager: Arc<TunnelManager>,
    db: DatabaseConnection,
}

// WebSocket Tunnel Management
struct TunnelManager {
    active_tunnels: HashMap<Uuid, WebSocketTunnel>, // cluster_id -> tunnel
    connection_pool: HashMap<String, TcpStream>,    // tunnel_id -> tcp_connection
}

// Preview URL Handler
async fn handle_preview_url(
    subdomain: &str,
    req: HttpRequest,
    proxy: &PreviewUrlProxy,
) -> Result<HttpResponse, ProxyError> {
    // 1. Parse subdomain
    let route_info = parse_preview_url(subdomain)?;
    
    // 2. Database lookup
    let environment = get_environment(route_info.env_hash).await?;
    
    // 3. Validate access
    validate_access(&req, &environment).await?;
    
    // 4. Get tunnel for cluster
    let tunnel = proxy.tunnel_manager.get_tunnel(environment.cluster_id).await?;
    
    // 5. Open TCP connection over tunnel
    let target = format!("{}.{}.svc.cluster.local", route_info.service, environment.namespace);
    let tunnel_id = open_tunnel_connection(tunnel, target, route_info.port).await?;
    
    // 6. Forward HTTP request/response
    forward_tcp_over_tunnel(req, tunnel, tunnel_id).await
}
```

#### **KubeManager Components:**
```rust
// WebSocket Tunnel Client
struct TunnelClient {
    websocket: WebSocketStream,
    active_connections: HashMap<String, TcpStream>, // tunnel_id -> tcp_stream
    message_handler: TunnelMessageHandler,
}

// Tunnel Message Handler
async fn handle_tunnel_message(
    msg: TunnelMessage,
    connections: &mut HashMap<String, TcpStream>,
) -> Result<(), TunnelError> {
    match msg {
        TunnelMessage::OpenConnection { tunnel_id, target_host, target_port } => {
            let stream = TcpStream::connect((target_host, target_port)).await?;
            connections.insert(tunnel_id.clone(), stream);
            
            // Send confirmation back
            send_message(TunnelMessage::ConnectionOpened { tunnel_id }).await?;
        },
        
        TunnelMessage::Data { tunnel_id, payload } => {
            if let Some(stream) = connections.get_mut(&tunnel_id) {
                stream.write_all(&payload).await?;
            }
        },
        
        TunnelMessage::CloseConnection { tunnel_id } => {
            connections.remove(&tunnel_id);
        },
        
        _ => {} // Handle other messages
    }
    
    Ok(())
}
```

## 🎯 **Success Criteria**

- [ ] Preview URLs work for HTTP services
- [ ] WebSocket connections properly upgraded
- [ ] Multiple concurrent connections supported
- [ ] Authentication and access control enforced
- [ ] Monitoring and error handling in place
- [ ] Production-ready deployment configuration

## 📊 **Implementation Priority Matrix**

### **Priority 1: Foundation (Required for MVP)**
```
┌─────────────────────────────────────────────────────────┐
│ CRITICAL PATH - All items must be completed in order   │
├─────────────────────────────────────────────────────────┤
│ 1. Extend RPC traits (kube-rpc/lib.rs)                 │
│    └── Dependencies: None                               │
│    └── Effort: 1-2 days                                │
│                                                         │
│ 2. Implement TunnelRegistry (KubeController)            │
│    └── Dependencies: #1                                 │
│    └── Effort: 2-3 days                                │
│                                                         │
│ 3. Basic WebSocket tunnel (KubeManager)                 │
│    └── Dependencies: #1, #2                            │
│    └── Effort: 3-4 days                                │
│                                                         │
│ 4. Preview URL parsing & DB integration                 │
│    └── Dependencies: None (parallel with #1-3)         │
│    └── Effort: 2-3 days                                │
│                                                         │
│ 5. End-to-end HTTP proxy                                │
│    └── Dependencies: #2, #3, #4                        │
│    └── Effort: 2-3 days                                │
└─────────────────────────────────────────────────────────┘
```

### **Priority 2: Security & Reliability (Required for Production)**
```
┌─────────────────────────────────────────────────────────┐
│ SECURITY & RELIABILITY - Can be developed in parallel  │
├─────────────────────────────────────────────────────────┤
│ 6. Access control & authentication                      │
│    └── Dependencies: #4, #5                            │
│    └── Effort: 3-4 days                                │
│                                                         │
│ 7. Error handling & connection management               │
│    └── Dependencies: #3, #5                            │
│    └── Effort: 2-3 days                                │
│                                                         │
│ 8. Rate limiting & DDoS protection                      │
│    └── Dependencies: #5, #6                            │
│    └── Effort: 2-3 days                                │
└─────────────────────────────────────────────────────────┘
```

### **Priority 3: Production Features (Nice to Have)**
```
┌─────────────────────────────────────────────────────────┐
│ PRODUCTION FEATURES - Can be added incrementally       │
├─────────────────────────────────────────────────────────┤
│ 9. Monitoring & metrics                                 │
│    └── Dependencies: #5, #7                            │
│    └── Effort: 2-3 days                                │
│                                                         │
│ 10. WebSocket upgrade support                           │
│     └── Dependencies: #5                               │
│     └── Effort: 1-2 days                               │
│                                                         │
│ 11. Infrastructure & DNS setup                          │
│     └── Dependencies: #5 (for testing)                 │
│     └── Effort: 1-2 days                               │
└─────────────────────────────────────────────────────────┘
```

### **Development Timeline (Estimated)**
```
Week 1: Foundation Implementation
├── Day 1-2: RPC trait extensions (#1)
├── Day 3-5: TunnelRegistry & basic infrastructure (#2)
└── Day 6-7: WebSocket client setup (#3)

Week 2: Core Functionality  
├── Day 1-3: Preview URL parsing & DB integration (#4)
├── Day 4-7: End-to-end HTTP proxy (#5)

Week 3: Security & Reliability
├── Day 1-4: Access control implementation (#6)
├── Day 5-7: Error handling & connection mgmt (#7)

Week 4: Production Readiness
├── Day 1-3: Rate limiting (#8)
├── Day 4-5: Monitoring (#9)
├── Day 6-7: Final testing & deployment (#10, #11)
```

### **Dependency Graph**
```
┌─────────┐    ┌─────────────────┐    ┌─────────────────┐
│ RPC     │───▶│ TunnelRegistry  │───▶│ End-to-End     │
│ Traits  │    │ (Controller)    │    │ HTTP Proxy     │
│ (#1)    │    │ (#2)            │    │ (#5)           │
└─────────┘    └─────────────────┘    └─────────────────┘
     │                │                         ▲
     │                │         ┌───────────────┤
     │                ▼         │               │
     │         ┌─────────────────┐              │
     │         │ WebSocket Tunnel│              │
     │         │ (Manager) (#3)  │──────────────┘
     │         └─────────────────┘
     │
     ▼
┌─────────────────┐
│ Preview URL     │
│ Parsing (#4)    │────────────────────┐
└─────────────────┘                    │
                                       ▼
                                ┌─────────────────┐
                                │ Access Control  │
                                │ (#6)            │
                                └─────────────────┘
```

## 🚀 **Getting Started (Updated)**

### **Immediate Next Steps:**
1. **Start with RPC Extensions** (`crates/kube-rpc/src/lib.rs`)
   - Add tunnel management methods to both `KubeClusterRpc` and `KubeManagerRpc`
   - Define all data structures (`TunnelRegistrationResponse`, `TunnelStatus`, etc.)

2. **Implement TunnelRegistry** (`crates/kube/src/server.rs`)  
   - Add in-memory tunnel connection management to `KubeClusterServer`
   - Implement tunnel registration and heartbeat methods

3. **Basic WebSocket Client** (`crates/kube-manager/src/manager.rs`)
   - Add WebSocket tunnel establishment logic
   - Implement basic message handling for TCP tunneling

4. **Preview URL Database Methods** (`crates/db/api/src/lib.rs`)
   - Add required database lookup methods for preview URL resolution
   - Test with existing database schema

### **Development Environment:**
```bash
# Run specific builds for fast iteration
cargo check --bin lapdev-kube-manager    # Manager development
cargo check --lib -p lapdev-kube         # Controller development  
cargo check --lib -p lapdev-kube-rpc     # RPC development
cargo check --lib -p lapdev-db           # Database integration

# Integration testing
cargo run --bin lapdev-kube-manager       # Test tunnel client
# (Controller testing requires full setup)
```

## 📝 **Notes**

- Uses existing RPC infrastructure for control plane
- Leverages existing database schema for preview URLs
- Maintains security boundaries and network topology constraints
- Supports any TCP-based protocol (HTTP, WebSocket, gRPC, databases, etc.)
- Provides foundation for future tunneling features