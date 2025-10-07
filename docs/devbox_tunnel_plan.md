# Devbox Tunnel Implementation Plan

## Goals
- Deliver a CLI experience that lets a developer run a `lapdev devbox …` command on their workstation and establish a bi-directional tunnel with their Kubernetes cluster.
- **Primary Use Case**: Enable microservices development by intercepting service traffic so a developer can run one service locally while it integrates with other services running in the cluster.
- Reuse the existing tunnel infrastructure (WebSocket control + data plane, `TunnelRegistry`, TCP framing) so preview URLs and devbox tunnels share code paths.
- Support both traffic directions: Kubernetes workloads reaching services running on the devbox, and the devbox reaching in-cluster services.
- Maintain security boundaries (token-based auth, RBAC checks) and provide observability (metrics + logs) comparable to preview URL tunnels.

## Core Use Case: Service Interception
A developer has a microservices application running in their Kubernetes environment (e.g., frontend, auth-service, payment-service in `prod-alice`). They want to:
1. Work on `auth-service` locally on their devbox
2. Run `lapdev devbox connect` to establish control/data-plane WebSocket sessions (this succeeds even if no environment is currently selected).
3. In the Lapdev web UI, choose `prod-alice` as the active environment for devbox work (can be done before or after connecting).
4. In the Lapdev web UI, open the Devbox panel and start an intercept for `auth-service`, targeting local port 8080 on the connected device.
5. The Lapdev API delivers the intercept assignment to the active CLI session, which binds `localhost:8080` and signals readiness.
6. Traffic from other cluster services to `auth-service` gets routed to the developer's local machine, while the local service can still call other cluster services within the environment.
7. On disconnect, UI stop, or Ctrl+C, traffic automatically restores to the original cluster service.

## Terminology
- **Devbox**: the developer workstation running the CLI; previously referred to as "local laptop".
- **Environment**: a Kubernetes namespace/cluster owned by a specific user; users only have access to their own environments (e.g., `prod-alice`, `dev-alice`).
- **Devbox Session**: a long-lived CLI process (one per user at a time) that keeps control/data-plane WebSockets open. Environment selection happens separately via the web UI when intercepts are initiated.
- **Cluster endpoint**: the existing kube-manager agent running inside the Kubernetes cluster.
- **Control plane**: Lapdev API server endpoints that broker tunnel registrations and RPC.
- **Data plane**: the existing `/kube/cluster/tunnel` WebSocket used to forward framed TCP payloads.
- **Tunnel**: a logical TCP pipe identified by a `tunnel_id`; currently created by preview URL proxy traffic.

## Architecture Overview
### Current State
1. Kubernetes clusters connect to Lapdev API via two WebSockets: control-plane RPC (`/kube/cluster/rpc`) and data-plane tunneling (`/kube/cluster/tunnel`).
2. `TunnelRegistry` inside the API keeps per-cluster senders, pending request channels, and data channels for active tunnels.
3. Preview URL proxy opens tunnels by sending `ServerTunnelMessage::OpenConnection` to kube-manager; data then flows through `TunnelRegistry`.

### Target State With Devbox
1. A devbox CLI process opens its own pair of WebSockets to the API (mirroring kube-manager) and authenticates with a user-scoped token (cluster access enforced via RBAC).
2. Devbox CLI connects through the dedicated devbox tunnel endpoint while kube-manager remains on the cluster tunnel endpoint; both authenticate with their respective tokens and reuse the existing bi-directional tunnel framing.
3. `TunnelRegistry` tracks both cluster agents and devbox sessions so tunnels can be routed between them.
4. When a tunnel is requested:
   - **Devbox → Cluster**: CLI asks API to open a tunnel targeting a Kubernetes service; API forwards an `OpenConnection` to kube-manager. Data plane routes bytes between devbox data socket and cluster TCP stream.
  - **Cluster → Devbox (Service Interception)**: Web UI drives intercept creation; CLI receives `StartIntercept`, calls the service interception RPC, and kube-manager updates the workload's existing `kube-sidecar-proxy` configuration to target the developer's devbox; traffic flows: K8s Service → Sidecar Proxy Container → WebSocket Tunnel → Devbox CLI → Local Process. `TunnelRegistry` brokers the data plane between sidecar and devbox over the existing tunnel framing.
5. The API acts as broker only; no direct devbox ↔ kube-manager WebSocket is required.

### Service Interception Architecture
**Strategy: Sidecar Proxy Config Update (Reuse Existing Image)**

This approach provides clean semantics without requiring a service mesh:

1. **Environment Selection**: Developer ensures `prod-alice` is active for devbox use by choosing it in the Lapdev web UI whenever they plan to intercept traffic (this selection can happen before or after starting the CLI connection).
2. **Devbox Session**: Developer runs `lapdev devbox connect` to establish control/data-plane WebSockets (no environment required at connect time).
3. **Intercept Request**: Developer uses the Lapdev web UI to select `auth-service`, choose local port 8080, and start an intercept against the currently connected device.
4. **API Notification**: Control plane validates the request, pushes a `StartIntercept` message to the devbox CLI session, and waits for the CLI to reserve the requested local port.
5. **Config Update**: kube-manager generates a `DevboxTunnel` route for the workload's existing sidecar proxy by:
   - Creating/patching the sidecar ConfigMap (`proxy-config.yaml`) with a route that points to the developer's devbox (local port + intercept metadata).
   - Applying/updating the `lapdev.io/proxy-*` annotations / labels on the Service/Workload (similar to branch routing) so the sidecar watches pick up the change.
6. **Sidecar Reload**: The injected `kube-sidecar-proxy` container (already running alongside the workload) receives the updated configuration via its watchers, registers the intercept with `SidecarProxyManager`, and obtains a devbox tunnel assignment from kube-manager.
7. **Traffic Flow**:
   - Cluster traffic → K8s Service → existing workload pods (iptables rules forward requests into the sidecar container)
   - Sidecar Proxy → establishes / reuses a devbox tunnel via kube-manager → Lapdev API → Devbox CLI
   - Devbox CLI → Local TCP socket (port 8080) → Developer's local auth-service process
8. **Cleanup**: On devbox disconnect or explicit stop, kube-manager restores the original sidecar configuration (removing the `DevboxTunnel` route), clears Service annotations, revokes the intercept token, and ensures the sidecar resumes routing traffic to the in-cluster workload.

**Why This Approach:**
- Leverages the existing `kube-sidecar-proxy` implementation already coupled with Lapdev infrastructure (ConfigMap-driven routing, service discovery, RPC connection to kube-manager).
- Works with vanilla Kubernetes (no service mesh required).
- Config/annotation patching mirrors existing branch-environment behavior and can be rolled back atomically.
- Clear observability — pod + service annotations expose intercept metadata.
- Low overhead (reuse of shipping sidecar image, no new binary).
- Keeps transport concerns in one place: sidecar already resolves iptables `SO_ORIGINAL_DST` addresses, performs protocol detection, and will be extended to open the devbox tunnel via kube-manager.
- Aligns with existing branch-environment routing: we reuse the same sidecar ConfigMap/annotation machinery (`lapdev.io/routing-key`, `proxy-config.yaml`) so workload-level behavior remains consistent.

## Protocol & Registry Updates
1. **Authentication**
   - Cluster agents authenticate via cluster tokens (existing mechanism) on `/kube/cluster/tunnel` endpoint.
   - Devbox CLI authenticates via Lapdev session token (`Authorization: Bearer <session_token>`) on dedicated `/kube/devbox/tunnel` endpoint.
   - Separate WebSocket endpoints (`/kube/cluster/tunnel`, `/kube/devbox/tunnel`) identify the caller role; no `endpoint_role` payload field needed.
   - With dedicated endpoints the tunnel protocol can remain unchanged; no capability flags are required beyond the existing message types.
   - Token validation on devbox endpoint: extract bearer token, look up hash in `devbox_session`, verify not expired/revoked, load scope metadata and associated `oauth_connection`.
2. **Tunnel Routing**
   - Augment `TunnelRegistry` to maintain separate sender maps for clusters and devboxes: `cluster_senders`, `devbox_senders` keyed by `(cluster_id, session_id)`.
   - Track active devbox sessions with metadata (session id, user id, organization id, environment/namespace, device name) pulled from authentication payload.
   - Each devbox session has a unique session_id; the API enforces only one active session per user (last session wins).
3. **Message Extensions**
   - Add `ServerTunnelMessage::OpenInterceptConnection { tunnel_id, service_name, namespace, source_addr, target_port, intercept_id }` for service interception traffic.
   - Add `ServerTunnelMessage::SessionDisplaced { reason, new_session_id, new_device_name }` to notify old sessions of displacement.
   - Extend existing `OpenConnection` to support `target_endpoint` enum for devbox-targeted connections.
   - Ensure `ConnectionOpened/Failed/Data/CloseConnection` share the same shape regardless of endpoint role.
4. **Lifecycle Management**
   - `TunnelRegistry::register_data_channel` stores channels per tunnel independent of endpoint role; cleanup must remove entries on disconnect from either side.
   - Heartbeats (`tunnel_heartbeat`) should accept devbox sessions and update presence.

## CLI Design (`lapdev devbox`)
1. **Authentication Commands**
   - `lapdev devbox login [--device-name "My MacBook"]` — initiates OAuth device flow, opens browser for SSO authentication (user selects GitHub/GitLab in browser), stores tokens in keychain.
   - `lapdev devbox logout` — revokes current session and removes tokens from keychain.
   - `lapdev devbox whoami` — displays current session info (user, org, provider, scopes, device name, expiry).
   - `lapdev devbox sessions list` — displays all active devbox sessions for current user (session ID, device name, environment, created/last used/expires at).
   - `lapdev devbox sessions revoke <session-id>` — revokes a specific session.
2. **Environment Connection Commands** (Primary Workflow)
   - `lapdev devbox connect` — establish control/data-plane WebSocket sessions for the devbox. This succeeds even if no environment has been selected yet.
   - Once connected, the devbox can accept intercept assignments for whichever environment the user designates in the web UI.
   - Connection persists until explicit disconnect or session termination.
   - CLI process remains running to keep control-plane and data-plane WebSockets alive; `lapdev devbox connect` blocks until the user disconnects (via `lapdev devbox disconnect`, Ctrl+C, or displacement).
   - `lapdev devbox disconnect` — disconnect from current environment (if any) and clean up all intercepts.
   - `lapdev devbox status` — show connection state, including whether an environment is currently selected, along with active intercepts and traffic stats.
   - **Environment Selection**: Managed via the web UI. Selecting/changing an environment updates server state; intercept commands use the latest selection without requiring a reconnect.
3. **Service Interception Experience** (Primary Use Case)
   - **Prerequisites**: Must be running `lapdev devbox connect`; environment selection is handled via the web UI and can be changed at any time.
   - **Initiation**: Intercepts are created from the Lapdev web UI (Devbox tab). User selects service, local port, and connected device, then starts the intercept.
   - CLI session receives a `StartIntercept` event, validates local port availability, and begins proxying without additional CLI flags.
   - `lapdev devbox intercept list` — show all active intercepts for current session.
   - `lapdev devbox intercept stop <intercept-id>` — stop specific intercept and restore service (UI also exposes stop controls).
   - `lapdev devbox intercept stop --all` — stop all intercepts.
   - **Automatic Displacement** (Same User, Same Environment):
     - If user connects from a different device to the same environment, old session is automatically displaced.
     - Old session receives notification and terminates gracefully.
   - **Optional Flags**:
  - `--no-displace` — Fail if the user already has an active devbox session instead of displacing it.
     - `--force` — Immediately displace old session without 2-second grace period.
   - **Phase 2 Features**:
     - `lapdev devbox logs --intercept <id>` — show HTTP request logs for intercepted traffic.
4. **Runtime Behavior**
   - Load Lapdev session token from keychain; check expiry and prompt re-login if expired.
   - **Environment Connection**:
     - On `connect` command, establish control/data plane WebSockets (without requiring environment metadata).
     - Store connection state in `~/.lapdev/session.json`, including the last-known environment (if any) for UX purposes.
   - Start local TCP listeners/outbound connectors per requested mapping.
   - Relay data frames via `TunnelRegistry` using existing framing (`ServerTunnelFrame`/`ClientTunnelFrame`).
   - Handle multiple concurrent tunnels within the environment using tokio tasks; graceful shutdown on SIGINT.
   - **Service Interception Behavior**:
     - When the API receives an intercept request from the web UI, it sends a `StartIntercept` control message to the CLI with service and port details.
     - CLI reserves the requested local port, acknowledges readiness, and calls `register_service_intercept` RPC to kube-manager with current environment context.
    - CLI immediately establishes the WebSocket tunnel and starts the local TCP listener after sending the RPC; kube-manager subsequently patches the workload's sidecar configuration (ConfigMap + annotations) to add a `DevboxTunnel` route and handles retries as needed.
     - On UI stop, Ctrl+C, or `disconnect`, CLI calls `unregister_service_intercept` to restore all services.
     - Automatic cleanup on WebSocket disconnect via `TunnelRegistry::cleanup_devbox_session`.
6. **UX**
   - Structured logging with `tracing`; progress output for users (connected, bytes transferred, session info).
   - Device flow displays user code prominently with clear instructions: "Visit https://lapdev.example.com/device and enter code: ABCD-EFGH"
   - Auto-open browser on supported platforms; fallback to manual URL copy.
   - Optional `--once` to close after first connection; default keep-alive.
   - **Environment Connection UX**:
     - `lapdev devbox connect` shows connection progress and reports whether an active environment is currently selected (including namespace/cluster details when available).
     - If no environment is selected, CLI remains connected but logs: "⚠️  No environment selected. Choose one in the Lapdev web UI at https://lapdev.example.com/devbox to enable intercepts."
     - When an environment is selected: "✓ Environment set to prod-alice (namespace: prod-alice, cluster: cluster-123)"
     - On automatic displacement: "✓ Found existing session from 'MacBook Pro'. Gracefully terminated. ✓ Connected from Work Laptop"
     - On `--no-displace` conflict: "⚠️  This user already has an active devbox session from 'MacBook Pro'. Use --force to displace or disconnect from the other device."
     - Environment can be changed in UI at any time; subsequent `connect` picks up the new selection.
   - **Service Interception UX**:
     - Terminal session streams real-time connection logs whenever an intercept is started from the web UI (requests received, forwarded to local port).
     - Clear status messages: "✓ Intercepting auth-service:8080 → localhost:8080" with traffic counter.
     - Prerequisites check: "⚠️  Not connected to an environment. Run 'lapdev devbox connect', then choose an environment in the Lapdev web UI before starting an intercept." (emitted if UI attempts an intercept without an active CLI connection or environment selection).
     - Graceful shutdown message: "🛑 Restoring service auth-service... ✓ Service restored"
7. **Packaging**
   - Add Clap-based subcommand to top-level `lapdev` binary (requires refactoring the existing `src/main.rs` to parse CLI before starting API server or split into separate binaries). Recommend creating a dedicated `lapdev-devbox` binary initially to avoid impacting server startup path.

## Server/API Changes
1. **Authentication & Session Tracking**
   - Implement OAuth 2.0 device flow endpoints in `crates/api/src/device.rs` (`device_code`, `device_token`, `device_approve`).
   - Add device authorization web page (`/device`) integrated with existing SSO; reuse `Auth` module and `oauth_connection` flow.
   - Validate Lapdev session tokens in dedicated devbox WebSocket handler (`crates/api/src/devbox.rs::devbox_tunnel_websocket`) by looking up token hash in `devbox_session` table.
   - **`devbox_session` table**: Stores long-lived login sessions (30 days) created during OAuth device flow. Includes `active_environment_id` field (nullable) reflecting the user's latest selection in the web UI. One session can connect to multiple environments over time.
   - **Active Environment Selection**: Maintained via web UI updates to `devbox_session.active_environment_id`. `lapdev devbox connect` can proceed even when this field is `NULL`; intercept operations that require an environment will return a clear error if none is selected.
   - **`DevboxRegistry` (in-memory)**: Tracks active devbox sessions. Stores (session id, user id, optional environment id, oauth_connection_id, device name, WebSocket sender, connected_at timestamp). Only one active session per user is allowed.
   - **Displacement Semantics**: Last-session-wins is enforced at connect time:
     1. CLI connects; API looks up any existing connection for the same user in `DevboxRegistry`.
     2. If found, API sends `SessionDisplaced` to the old WebSocket, waits up to 2 seconds for graceful cleanup, then force-closes if needed.
     3. API triggers cleanup of intercepts owned by the displaced session.
     4. New connection is registered in `DevboxRegistry` as the active session for that user.
   - Extend `CoreState` to hold both `TunnelRegistry` and `DevboxRegistry` (or merge into enhanced `TunnelRegistry`).
   - Mint short-lived `devbox_intercept_token` values whenever an intercept is registered; tokens are scoped to `(intercept_id, devbox_session_id)` and used by both CLI (for tunnel auth) and sidecar containers (via env var) when opening data-plane connections.
2. **Routing Logic**
   - Update data-plane handler to distinguish messages originating from clusters vs devboxes; forward `ClientTunnelMessage` to the opposite party based on `tunnel_id` metadata.
   - Permit `ServerTunnelMessage::OpenConnection` requests from devboxes targeting clusters and vice versa.
   - Ensure cleanup removes tunnel data channels and notifies counterpart on disconnect to avoid dangling waits in CLI.
3. **API Surface**
   - Optional REST endpoints to list active devbox sessions and tunnels for observability.
4. **Metrics**
   - Extend `TunnelRegistry::update_metrics` to capture endpoint type; export metrics per role (e.g., `devbox_bytes_transferred`).

## Kube-Manager Changes
1. **Service Interception Implementation**
   - Extend the existing `ServiceInterceptor` to manage (building on the branch-environment routing flow):
     - Creation/patching of the workload's sidecar ConfigMap (`proxy-config.yaml`) with a `DevboxTunnel` target (mirroring how branch routes are encoded today).
     - Application of `lapdev.io/proxy-*` annotations/labels so injected sidecars reload without restarting pods.
     - Registration bookkeeping with `SidecarProxyManager` (existing component) so kube-manager can broker tunnel requests back to the API.
   - **Key Methods**:
    - `intercept_service_full()` — backs up the Service/sidecar configuration, patches the ConfigMap + annotations with the `DevboxTunnel` route, and stores intercept metadata.
    - `restore_service()` — restores the prior proxy configuration/annotations and removes intercept tracking.
     - `list_interceptable_services()` — returns services available for interception in the namespace (excluding those already intercepted).
   - **Note**: Displacement logic lives in API server `DevboxRegistry`; kube-manager only owns proxy configuration lifecycle per intercept.
   - **State Tracking**:
     - Maintain `active_intercepts` map: `Uuid → InterceptState` containing service name, namespace, original proxy config snapshot, ConfigMap name, routing key, devbox session id, hashed sidecar auth token, and the currently assigned tunnel id.
     - Persist intercept metadata in `devbox_service_intercept` so kube-manager can recover state after restarts and reapply proxy configuration (including regenerating tokens when required).
   - **Cleanup Logic**:
    - On devbox WebSocket disconnect, API calls `unregister_service_intercept`; kube-manager restores the prior proxy configuration, removes the `DevboxTunnel` route, and deletes intercept state.
    - Background job checks for stale `DevboxTunnel` routes / ConfigMaps without active intercept rows and cleans them up.
     - Heartbeat mechanism leverages `SidecarProxyManager` RPC (`heartbeat`, `report_routing_metrics`) to detect stuck sidecars (>90s) and trigger restoration.
2. **Sidecar Proxy Configuration (Reuse)**
   - Reuse existing injected `lapdev/kube-sidecar-proxy` containers; no additional pods are created.
   - Extend sidecar configuration semantics to include a `DevboxTunnel` route entry carrying `intercept_id`, `devbox_session_id`, `target_port`, and a short-lived devbox auth token.
   - Ensure the sidecar continues to mount/consume `/etc/lapdev/proxy/proxy-config.yaml` (or equivalent) and reacts to updates via the current discovery/watch pipeline.
   - Maintain access to `SidecarProxyManager` via the existing ClusterIP service (`LAPDEV_SIDECAR_PROXY_MANAGER_ADDR`).
3. **Sidecar Proxy RPC / Tunnel Plumbing**
   - Implement the `SidecarProxyManagerRpc::heartbeat` and `report_routing_metrics` handlers to update intercept state and surface health in kube-manager.
   - Add a new RPC (or extend `register_sidecar_proxy`) enabling sidecar instances to request `DevboxTunnelConnection { intercept_id, source_addr, target_port }` when a `DevboxTunnel` route is activated.
   - On each request, kube-manager uses the existing `TunnelManager` to issue `ServerTunnelMessage::OpenConnection` toward the Lapdev API, pairing the resulting `tunnel_id` with the requesting sidecar and streaming bytes between the Tungstenite connection and the sidecar socket.
   - Track active sidecar → devbox streams so forced cleanup (displacement, CLI disconnect, pod restart) can close outstanding tunnels.
4. **RPC Methods**
   - Implement new `KubeClusterRpc` methods:
     - `register_service_intercept(devbox_session_id, request)` — creates intercept, returns intercept_id and tunnel_id.
    - `unregister_service_intercept(intercept_id)` — reverts proxy configuration and clears intercept state.
     - `list_interceptable_services(namespace)` — returns services with port info and intercept status.
   - Add authentication checks: verify devbox session has RBAC permission for target namespace.
5. **Configuration**
   - No additional capability flags needed initially; service interception is a standard feature.
   - Future: capability flag for advanced features (e.g., header routing) in Phase 2.

## Devbox CLI Networking Flows
### Devbox → Kubernetes (Local Forward)
1. User runs CLI specifying remote target (`service.namespace:port`).
2. CLI sends `OpenConnection` to API; API forwards to kube-manager via existing channel.
3. kube-manager opens TCP connection to service, responds with `ConnectionOpened`.
4. CLI streams local socket bytes via `ServerTunnelMessage::Data`; responses from kube-manager routed back to CLI and written to local client.

### Kubernetes → Devbox (Service Interception - Primary Use Case)
1. **Setup Phase**:
   - **Prerequisites**:
     - User has an active `lapdev devbox connect` session.
     - User has selected `prod-alice` as the active environment in the Lapdev web UI (if not, the intercept request will fail with a clear error).
   - User initiates intercept from Lapdev web UI (Devbox → Start Intercept) for `auth-service`, mapped to local port 8080 on the connected device.
   - API pushes `StartIntercept` message to CLI; CLI validates local port availability, reserves it, and acknowledges readiness.
   - CLI posts `register_service_intercept` RPC to kube-manager with service name, current environment context, and requested local port, then immediately moves on to local tunnel setup.
   - API validates that an active environment is selected for the session; if not, it returns a descriptive error to the UI/CLI and does not contact kube-manager.
   - kube-manager:
     - Captures the existing sidecar proxy configuration for the workload (for later restoration).
     - Generates or patches the workload's `proxy-config.yaml` ConfigMap with a `DevboxTunnel` route pointing at the developer's devbox/local port and encodes intercept metadata (session id, intercept id, tunnel token).
     - Applies `lapdev.io/proxy-*` annotations/labels so the injected sidecar container reloads configuration (mirroring branch workload routing).
     - Returns `intercept_id`, `tunnel_id`, and config backup handle to the CLI.
   - CLI establishes WebSocket tunnel and starts local TCP listener on port 8080 right after issuing the RPC (no additional waiting on sidecar state).
   - Displays: "✓ Intercepting auth-service:8080 → localhost:8080"

2. **Traffic Flow Phase**:
   - Cluster workload sends request to `auth-service:8080`.
   - Traffic hits the workload pods; per-existing iptables rules forward connections into the injected sidecar container.
   - The sidecar recognizes the `DevboxTunnel` route, requests a devbox stream from kube-manager via `SidecarProxyManagerRpc`, and begins forwarding bytes over the established tunnel.
   - Kube-manager uses `TunnelRegistry` to broker an `OpenInterceptConnection { tunnel_id, service_name, namespace, source_addr, intercept_id }` toward the devbox session.
   - API routes the message to the devbox CLI via its active WebSocket.
   - CLI opens connection to local TCP port 8080 (developer's local auth-service process).
   - Bidirectional data flow:
     - Request: Sidecar Proxy → SidecarProxyManager/Kube-Manager → Lapdev API (TunnelRegistry) → Devbox CLI → Local Process
     - Response: Local Process → Devbox CLI → Lapdev API → Kube-Manager → Sidecar Proxy → Original Caller

3. **Cleanup Phase**:
   - User stops the intercept from the web UI, presses Ctrl+C, or runs `lapdev devbox intercept stop <intercept-id>`.
   - CLI calls `unregister_service_intercept(intercept_id)` RPC.
   - kube-manager:
     - Restores the previous sidecar proxy configuration (removing the `DevboxTunnel` route) and related annotations.
     - Revokes the devbox intercept token, clears tracking state, and signals the sidecar to close outstanding devbox streams.
   - CLI closes local TCP listener and WebSocket tunnel.
   - Displays: "✓ Service auth-service restored"

4. **Failure Handling**:
   - **Devbox WebSocket Disconnect**: API detects disconnect, calls `cleanup_devbox_session`, which triggers `unregister_service_intercept` for all active intercepts in that environment.
   - **Sidecar Stuck**: Background job monitors `SidecarProxyManager` heartbeats; missing heartbeats trigger config rollback and error surfacing to the CLI/UI.
   - **Environment Already Connected**: If user connects to same environment from different device, automatic displacement occurs (last session wins).
  - **Prerequisites Not Met**: If user tries to intercept without an active CLI connection or without selecting an environment in the web UI, return clear errors instructing them to connect and/or choose an environment before retrying.

### Kubernetes → Devbox (Reverse Forward - Future Use Case)
1. kube-manager requests `OpenConnection` for a devbox endpoint, providing desired `local_port` (cluster side) and `devbox_port`.
2. API looks up matching devbox session, forwards request.
3. CLI opens a TCP listener/outbound connection on the devbox (depending on mode) and acknowledges `ConnectionOpened` once ready.
4. kube-manager forwards in-cluster traffic; CLI writes payloads to local process, returning responses through API.

## Security Considerations
- Session tokens: bind Lapdev session tokens to user identity via SSO; scope constraints enforced at authorization time and validated on every WebSocket handshake.
- OAuth connection validation: verify associated `oauth_connection` is valid (not soft-deleted) on every WebSocket auth to detect SSO revocation.
- RBAC enforcement: intersect session scope with current user/org RBAC policies; reject tunnels targeting inaccessible clusters/namespaces.
- Token storage: use OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service) with fallback to AES-256-GCM encrypted file.
- Validation: API must verify that requested cluster IDs match authenticated permissions before tunnel brokerage.
- Rate limiting: add per-user tunnel limits to avoid resource exhaustion; track concurrent tunnels in `DevboxRegistry`.
- Token lifetime: session tokens expire after 30 days; no automatic refresh to limit credential lifetime.
- Encryption: rely on WSS/TLS for transport; session tokens hashed at rest with Argon2id; never logged in plaintext.
- SSO revocation: detect when user disconnects OAuth provider on next WebSocket auth; session rejected and user prompted to re-login.
- Device code security: device codes hashed at rest; 5-minute TTL to limit phishing window; single-use to prevent replay attacks.

## OAuth Device Flow Authentication
### Rationale & Requirements
- Reuse existing GitHub/GitLab SSO integration (`oauth_connection` table + auth module) for devbox CLI authentication, providing a seamless user experience.
- Leverage OAuth 2.0 device authorization grant (RFC 8628) for headless CLI authentication without embedding client secrets.
- Device flow initiates browser-based SSO that creates/updates existing `oauth_connection` entries; devbox sessions reference these connections.
- Support scoped access for devbox operations while maintaining auditability and RBAC enforcement.

### Device Flow Overview
1. User runs `lapdev devbox login` on their workstation.
2. CLI requests a device code from Lapdev API (`POST /v1/auth/device/code`).
3. API returns a `device_code`, `user_code`, and `verification_uri` (e.g., `https://lapdev.example.com/device`).
4. CLI displays the user code and opens browser to verification URI.
5. User enters the displayed user code on the web page.
6. Web page prompts user to select authentication provider (GitHub or GitLab).
7. User completes SSO authentication (reuses existing `session_authorize` flow); browser-based OAuth flow creates/updates `oauth_connection` entry with provider access token.
8. After successful SSO, user is redirected to approval page to select scope constraints (clusters, namespaces) and approves devbox access.
9. CLI polls the token endpoint (`POST /v1/auth/device/token`) until authorization completes.
10. API returns a Lapdev-issued devbox session token; CLI stores it securely in OS keychain.

### Data Model
- **Reuse existing `oauth_connection` table**: stores GitHub/GitLab OAuth tokens (no changes needed).
- **Create `device_authorization` table**: `id` (UUID PK), `device_code` (hashed, unique), `user_code` (unique), `expires_at`, `interval`, `authorized_user_id`, `authorized_oauth_connection_id`, `authorized_at`, `created_at`.
- **Create `devbox_session` table (Login Sessions)**: Stores long-lived login sessions created via OAuth device flow. One session can connect to multiple environments over time. Fields: `id` (UUID PK), `user_id`, `organization_id`, `oauth_connection_id` (FK to oauth_connection), `session_token_hash` (unique), `token_prefix`, `device_name`, `scopes` (text[]), `active_environment_id` (UUID nullable, FK to environments/clusters), `created_at`, `expires_at`, `last_used_at`, `revoked_at`, `metadata` (JSONB for cluster/namespace constraints).
- **Active Environment Selection**: `active_environment_id` field stores the user's currently selected environment. Set via web UI API endpoint (`PUT /v1/devbox/active-environment`) and consumed by intercept-related APIs; `lapdev devbox connect` does not depend on it, but intercept requests will fail if the field is `NULL`.
- **Create `devbox_service_intercept` table**: `id` (UUID PK), `session_id` (FK to devbox_session), `user_id` (FK to users), `cluster_id` (FK to kube_cluster), `namespace`, `service_name`, `target_port`, `local_port`, `intercept_mode` (default 'full'), `config_map_name`, `routing_key`, `sidecar_auth_token_hash`, `tunnel_id`, `original_proxy_config` (TEXT), `created_at`, `restored_at` (nullable).
- **Environment Connections (In-Memory Only)**: Tracked in `DevboxRegistry` struct in API server memory. Not persisted to database. Each entry represents the single active devbox session for a user (including metadata such as latest environment selection, device name, and WebSocket sender). Connecting from a new device replaces the previous entry.
- Session tokens are Lapdev-issued opaque tokens (hashed with Argon2id); include `token_prefix` field (e.g., `ldvbx_abcd`) for support.
- Indexes for `devbox_session`:
  - `(user_id, revoked_at)` for user session queries
  - `(oauth_connection_id, revoked_at)` for OAuth revocation cascade
  - `(last_used_at)` for cleanup job
  - `(session_token_hash)` unique for fast auth lookup
- Indexes for `device_authorization`:
  - `(device_code)` unique on device_authorization
  - `(user_code)` unique on device_authorization for approval lookup
- Indexes for `devbox_service_intercept`:
  - `(session_id)` for session queries
  - `(cluster_id)` for cluster queries
  - `(restored_at)` partial index for active intercepts
  - `(user_id, cluster_id)` for finding all intercepts by user in an environment (used during displacement)
  - `(routing_key)` unique to avoid duplicate routing assignments per service
  - `(sidecar_auth_token_hash)` unique for constant-time token lookup during tunnel handshakes
  - Note: Displacement happens at environment (cluster_id) level, not per-service. When a user connects to an environment from a new device, all intercepts in that environment are cleaned up via session_id cascade.

### Scope & Policy Model
- Define canonical scopes (`devbox:tunnel`, future `devbox:exec`, `devbox:port-forward`) validated at authorization time.
- During device authorization approval, user selects scope constraints (cluster IDs, namespace selectors) via web UI.
- Store constraints in `devbox_session.metadata` JSONB column; enforce RBAC intersection on every WebSocket handshake and tunnel operation.
- Session tokens are long-lived (30 days default); re-authentication required after expiry or revocation.

### API & CLI Surface
1. `POST /v1/auth/device/code` — initiates device flow, returns `device_code`, `user_code`, `verification_uri`, `interval`.
2. `GET /device` (web page) — device authorization approval page; user enters code, selects GitHub or GitLab provider, authenticates via SSO, selects scope constraints.
3. `POST /v1/auth/device/approve` — browser submits approval with user_code and scope constraints; associates device authorization with authenticated user.
4. `POST /v1/auth/device/token` — CLI polls for authorization; returns Lapdev session token once approved.
5. `GET /v1/devbox/sessions` — lists active devbox sessions for the authenticated user (requires browser session cookie).
6. `DELETE /v1/devbox/sessions/{session_id}` — revokes a session and disconnects active tunnels.
7. CLI commands: `lapdev devbox login`, `lapdev devbox logout`, `lapdev devbox whoami`, `lapdev devbox sessions` (`list`, `revoke`).
8. Token storage: OS keychain (macOS Keychain, Windows Credential Manager, Linux Secret Service) with fallback to encrypted file (`~/.lapdev/credentials.enc`).

### Observability & Operations
- Emit audit events: `device.initiated`, `device.authorized`, `devbox.session.created`, `devbox.session.revoked`.
- Track metrics: `device_authorizations_pending`, `devbox_sessions_active`, `devbox_session_age_seconds`.
- Background job sweeps expired device codes (5 min TTL) and expired session tokens (30 days default).
- Log all session usage with user, org, scope, and oauth_connection context for security auditing.

### Migration & Integration
- Add SeaORM entities and migrations for `device_authorization` and `devbox_session`.
- Reuse existing `oauth_connection` table; no schema changes required.
- Extend existing SSO callback handlers to support device authorization approval flow (new `/device` page and approval endpoint).
- Implement device flow endpoints in API (`crates/api/src/device.rs`).
- Update `DevboxRegistry` to track session IDs and oauth_connection IDs for revocation propagation.

## Devbox Session Implementation
### Token Format & Storage
- Devbox session tokens are Lapdev-issued opaque bearer tokens (not provider OAuth tokens).
- Include a short prefix (e.g., `ldvbx_`) on session tokens for log correlation and support diagnostics.
- CLI stores session token in OS keychain under service name `com.lapdev.devbox`; fallback to AES-256-GCM encrypted file (`~/.lapdev/credentials.enc`) with PBKDF2-derived key when keychain unavailable.
- Session token references `oauth_connection` (GitHub/GitLab OAuth token) which is used for git operations and provider API access.
- Scope metadata (cluster IDs, namespace selectors) captured during device approval and persisted in `devbox_session.metadata`.

### Scoping & Authorization
- During devbox WebSocket handshake (`/kube/devbox/tunnel`), extract bearer token from `Authorization` header.
- Look up token hash in `devbox_session`, verify not expired/revoked, and load scope + metadata.
- Load associated `oauth_connection` to verify user still has valid SSO; check `deleted_at` is null.
- Intersect session scope with current user RBAC policies; materialize resolved permissions in session context for constant-time tunnel authorization.
- Reject handshakes for expired, revoked, or scope-empty sessions; log rejection reason for audit trail.

### Login & Session Flow
1. User runs `lapdev devbox login [--device-name "My MacBook"]` (optionally with `--clusters c123,c456 --namespaces apps/dev` hints).
2. CLI initiates device flow (`POST /v1/auth/device/code`), displays user code and verification URL.
3. CLI opens system browser to `/device` page; user enters device code.
4. Web page prompts user to select authentication provider (GitHub or GitLab) and redirects to existing SSO flow (reuses `session.rs::new_session` + `session_authorize`).
5. After SSO completes and `oauth_connection` is created/updated, user is redirected to device approval page showing scope selection (clusters, namespaces).
6. User approves; browser submits `POST /v1/auth/device/approve` with user_code and scope constraints.
7. API associates device authorization with authenticated user and creates `devbox_session` entry linked to `oauth_connection`.
8. CLI polls `POST /v1/auth/device/token`; receives Lapdev session token once approved.
9. CLI stores session token in keychain and writes session metadata to `~/.lapdev/session.json` (session ID, user info, provider, expiry for UX).
10. Subsequent `lapdev devbox connect` commands and UI-triggered intercept instructions load session token from keychain and use it for WebSocket auth.

### Token Expiry & Validation
- Session tokens expire after 30 days (configurable); no automatic refresh (user must re-authenticate).
- Update `last_used_at` on every WebSocket auth; emit metrics for session age.
- On WebSocket auth, verify associated `oauth_connection` still exists and is not soft-deleted.
- If `oauth_connection` is deleted (user disconnected provider), reject auth and prompt re-login.

### Revocation & Cleanup
- User can revoke sessions via `lapdev devbox logout` (current session) or web UI / CLI `lapdev devbox sessions revoke <session_id>`.
- Revocation marks `revoked_at` in `devbox_session` and triggers `DevboxRegistry` notification to disconnect live tunnels.
- Background job sweeps expired session tokens hourly; logs revocations for security audit.
- SSO provider revocation (user disconnected OAuth provider) detected on next WebSocket auth; session rejected and user prompted to re-login.

### Bootstrapping & Migration
- Extend RBAC evaluation to consume scope metadata from `devbox_session.metadata` column.
- Add device authorization approval UI (new `/device` web page and approval endpoint) integrated with existing SSO flow.
- Provide admin CLI for emergency session issuance (`lapdev admin devbox create-session --user-id <uuid>`) that bypasses device flow but still respects RBAC.
- Document OAuth device flow in user docs with screenshots; provide troubleshooting guide for keychain and browser issues.

## Error Handling & Reliability
### WebSocket Reconnection
- CLI implements exponential backoff reconnection (initial 1s, max 60s) when WebSocket disconnects.
- Preserve session token across reconnections; prompt re-login only if token expired/revoked.
- On reconnect, re-establish all active tunnels automatically; notify user of tunnel restoration.
- Detect API server upgrades vs. network failures; adjust retry strategy accordingly.

### Tunnel Failure Handling
- When `ConnectionFailed` received, surface clear error to user with troubleshooting hints (check service name, namespace permissions, cluster connectivity).
- Implement tunnel-level timeouts: if `ConnectionOpened` not received within 30s, fail tunnel and notify user.
- Handle partial data transmission: if WebSocket closes mid-transfer, close local TCP sockets cleanly and log error.
- Detect and report tunnel ID collisions; regenerate tunnel IDs on collision.

### Rate Limiting & Backpressure
- API enforces per-user limits: max 10 concurrent devbox sessions, max 50 active tunnels per session.
- CLI implements TCP backpressure: pause reading from local socket if WebSocket send buffer fills.
- API enforces max WebSocket message size (1MB default) to prevent memory exhaustion.
- Return clear rate limit errors with retry-after hints when limits exceeded.

### Graceful Degradation
- If `oauth_connection` deleted during active session, allow existing tunnels to complete but reject new tunnel requests.
- When cluster becomes unreachable, notify devbox CLI immediately via `ConnectionFailed` rather than hanging.
- CLI supports `--timeout` flag to specify max tunnel idle time before auto-close.

## Observability & Diagnostics
- Extend structured logging with endpoint type and tunnel direction tags (`devbox_to_cluster`, `cluster_to_devbox`).
- Emit Prometheus metrics for active devbox sessions, tunnel counts, byte counters, error counts, reconnection events, rate limit hits.
- Provide CLI `--verbose` and `--log-file` options.
- Add tracing spans around tunnel lifecycle in both CLI and API; include correlation IDs for cross-component log correlation.
- CLI emits metrics to local endpoint (`http://localhost:9090/metrics`) when `--metrics` flag enabled for power users.

## Protocol Support & Multi-Developer Scenarios

### Protocol Support
**Phase 1: Raw TCP Proxying (MVP)**
- Forward raw TCP byte streams without protocol inspection.
- Works for HTTP, gRPC, database connections, and any TCP-based protocol.
- Simple implementation with minimal overhead.
- Sufficient for 90% of microservices use cases.

**Phase 2: HTTP-Aware Proxying (Enhanced Observability)**
- Parse HTTP requests in the sidecar proxy container.
- Inject custom headers for tracing and debugging:
  - `X-Lapdev-Intercept: <session_id>` — identifies intercepted traffic.
  - `X-Lapdev-Devbox: <hostname>` — shows which devbox handled request.
  - Preserve W3C Trace Context headers for distributed tracing.
- Log HTTP request details (method, path, status code) for debugging.
- Enable request/response metrics per endpoint.
- Graceful fallback to raw TCP for non-HTTP protocols (auto-detection).

**Phase 3: Advanced Protocol Features**
- gRPC-aware routing with method-level interception.
- WebSocket proxying with frame inspection.
- TLS termination in sidecar proxy for mTLS workloads.

### Multi-Developer Scenarios
**Architecture Principle**: Each developer has their own Kubernetes environments (e.g., `prod-alice`, `dev-alice`). Users **cannot** access other users' environments. Therefore, all conflicts are **same user, different devices** scenarios.

**Phase 1: Last Session Wins (Per User)**
- **Use Case**: Developer switches between devices (laptop, desktop, remote workstation).
- **Behavior**: Only one devbox CLI session per user may remain active. When a second device connects, the previous session (regardless of environment) is displaced automatically.
- **Displacement Flow**:
  1. User runs `lapdev devbox connect` from a new device (laptop).
  2. API sees an existing active session for that user in `DevboxRegistry`.
  3. API sends `SessionDisplaced` to the old WebSocket: `{ reason: "New connection from different device", new_device_name: "Work Laptop" }`.
  4. Old CLI cleans up local resources, closes the WebSocket, and logs the displacement message.
  5. API waits up to 2 seconds for graceful cleanup, then force-closes if needed.
  6. API triggers kube-manager RPCs to clean up intercepts owned by the displaced session.
  7. API registers the new connection in `DevboxRegistry` as the active session for that user.
  8. New CLI logs: `✓ Found existing session from "MacBook Pro". Gracefully terminated. ✓ Devbox session now owned by this device.`
- **Benefits**:
  - Seamless device switching without manual cleanup
  - Old session gets clear notification of displacement with specific environment and device info
  - Graceful shutdown with 2-second grace period
  - No orphaned resources or dangling tunnels
  - Simpler logic than per-service displacement
- **Database Enforcement**: Track the single active session per user; any new session automatically displaces the previous one.
- **Optional Flags**:
  - `--no-displace` — Fail if the user already has an active devbox session instead of displacing it
  - `--force` — Immediate displacement without grace period
- **Environment Switching**:
  - Because only one session per user is active, switching environments is done within that session via the Lapdev web UI.
  - Selecting a new environment in the UI tears down any active intercepts for the previous environment before new ones are created.
  - Within a single session, a user can intercept multiple services sequentially (or simultaneously if supported) inside the selected environment.

**No Cross-User Conflicts**:
- Users only have access to their own environments
- Environment names are per-user (e.g., `prod-alice` vs `prod-bob`)
- Cross-user conflicts are impossible by design
- No need for `--force` takeover of other users' sessions

**Phase 2: Session Sharing (Observability Only)**
- Allow developers to "observe" another developer's intercept without handling traffic.
- Read-only mode: see requests/responses but don't modify or handle traffic.
- Use case: Pair programming, debugging, training.

## Rollout Strategy
**Phase 1: MVP - Service Interception (2-3 weeks)**
1. Implement environment ownership + displacement (last session wins when a session claims an environment).
2. Implement devbox route configuration/annotation updates in kube-manager (reuse branch routing flow).
3. Update and publish existing `lapdev/kube-sidecar-proxy` image with `DevboxTunnel` routing support.
4. Add service intercept RPC methods to `KubeClusterRpc`.
5. Implement CLI `lapdev devbox connect` command and background intercept handler that reacts to UI-triggered requests (shared login flow).
6. Raw TCP proxying without protocol inspection.
7. Graceful cleanup on disconnect and proper error handling.
8. Database schema for `devbox_session` and `devbox_service_intercept`.
9. **Deliverable**: Developer can connect to their environment, intercept services, and route traffic to local machine with seamless device switching.

**Phase 2: Enhanced Features (2-3 weeks)**
1. HTTP-aware proxying with header injection and request logging.
2. Session sharing (observe-only mode) for pair programming and debugging.
3. Distributed tracing integration (W3C Trace Context propagation).
4. Metrics dashboard for intercept traffic (requests/sec, bytes, latency).
5. CLI improvements: `lapdev devbox logs --intercept <id>` for HTTP logs.
6. **Deliverable**: Production-ready observability and collaborative debugging workflows.

**Phase 3: Advanced Capabilities (3-4 weeks)**
1. Automatic failover to cluster on devbox disconnect (graceful degradation).
2. gRPC-aware routing and WebSocket proxying.
3. **Deliverable**: Enterprise features for large teams focused on resiliency and protocol coverage.

**Phase 4: Productionization**
1. Incorporate metrics dashboards and alerting thresholds for devbox traffic.
2. Admin UI for managing active intercepts across organization.
3. Audit logs for compliance and security review.
4. Performance optimization (reduce latency overhead).
5. Documentation, tutorials, and example workflows.

## Implementation Components Checklist

### Phase 1 MVP Components

**1. Database Schema** (`lapdev-db/`)
- [ ] Create migration for `device_authorization` table
- [ ] Create migration for `devbox_session` table:
  - Include `active_environment_id` field (nullable UUID, FK to environments/clusters)
  - Index on `(user_id, active_environment_id)` for quick active environment lookup
- [ ] Create migration for `devbox_service_intercept` table
- [ ] Generate SeaORM entities for new tables
- [ ] Add indexes for efficient queries

**2. RPC Definitions** (`lapdev-kube-rpc/`)
- [ ] Add `EnvironmentContext` struct (user_id, environment/namespace, cluster_id)
- [ ] Add `ServiceInterceptRequest` struct (service_name, target_port, port_name, intercept_mode) - no namespace field
- [ ] Add `ServiceInterceptResponse` struct
- [ ] Add `ServiceBackupState` struct
- [ ] Add `InterceptableService` struct
- [ ] Add `InterceptMode` enum (Full, ObserveOnly)
- [ ] Extend `KubeClusterRpc` trait with intercept methods:
  - `register_service_intercept(environment_context, request)` — context includes namespace
  - `unregister_service_intercept(intercept_id)`
  - `list_interceptable_services(environment_context)` — context includes namespace

**3. Sidecar Proxy Enhancements** (existing `lapdev/kube-sidecar-proxy`)
- [ ] Extend `ProxyConfig` / `RouteTarget` with a `DevboxTunnel` variant containing intercept id, devbox session id, tunnel auth token, and target port.
- [ ] Update `ServiceDiscovery` to hydrate `DevboxTunnel` routes from ConfigMap / Service annotations and push them through the broadcast channel.
- [ ] Enhance `handle_connection` / proxy pipeline to detect `DevboxTunnel` target and:
  - Request a data-plane connection via kube-manager (`SidecarProxyManagerRpc`) including intercept id + port.
  - Stream bytes between inbound socket and the tunnel (via `TunnelRegistry` frames) instead of connecting to `127.0.0.1`.
  - Preserve existing HTTP/TCP protocol detection for metrics/tracing.
- [ ] Implement heartbeat + metrics reporting on top of the existing RPC stubs (`heartbeat`, `report_routing_metrics`).
- [ ] Add readiness gating so kube-manager only proceeds once the sidecar registers and acknowledges the intercept config.
- [ ] Publish updated sidecar image (`lapdev/kube-sidecar-proxy:<tag>`) and update any preview URL manifests if flags are required.

**4. Kube-Manager Extensions** (`lapdev-kube-manager/`)
- [ ] Create `ServiceInterceptor` module that orchestrates ConfigMap + annotation lifecycle for intercepts
- [ ] `intercept_service_full()` implementation:
  - Backup relevant proxy configuration (ConfigMap contents, annotations) and persist snapshot in `devbox_service_intercept`
  - Mint short-lived `devbox_intercept_token`, patch ConfigMap (`proxy-config.yaml`) to embed the `DevboxTunnel` route, and update annotations/labels to trigger reload
  - Record `config_map_name`, `routing_key`, and hashed token in intercept state map; monitor sidecar readiness via `SidecarProxyManager` heartbeats and log/notify asynchronously if registration fails
- [ ] `restore_service()` implementation:
  - Restore original annotations/config map content
  - Revoke intercept token and notify sidecar/CLI of completion
  - Remove intercept entry from memory + mark DB record `restored_at`
- [ ] `list_interceptable_services()` filters by namespace and excludes services with active intercepts
- [ ] Expose RPC handlers (`register_service_intercept`, `unregister_service_intercept`, `list_interceptable_services`) and forward devbox tunnel frames via `TunnelManager`
- [ ] Implement background cleanup for stale `DevboxTunnel` routes / ConfigMaps and token expiration sweep
- [ ] Integrate `SidecarProxyManager` heartbeat/metrics into kube-manager telemetry (emit warnings when stale)

**5. API Server Extensions** (`lapdev-api/`)
- [ ] Create `crates/api/src/device.rs` for OAuth device flow
- [ ] Implement device flow endpoints:
  - `POST /v1/auth/device/code`
  - `POST /v1/auth/device/token`
  - `POST /v1/auth/device/approve`
  - `GET /device` (web page)
- [ ] Create `/kube/devbox/tunnel` WebSocket handler for devbox sessions
- [ ] Implement devbox session token validation:
  - Look up token in `devbox_session` table (login sessions)
  - Verify not expired/revoked
  - Load user_id, oauth_connection_id, scopes
- [ ] Create `DevboxRegistry` struct (in-memory) for tracking the single active devbox session per user:
  - Map: `user_id → DevboxSessionEntry`
  - Store: session_id, user_id, latest environment/cluster id (optional), device name, WebSocket sender, connected_at
  - Provide helpers to fetch the active connection for a user and to enumerate connections for ops/debugging
- [ ] Implement connection flow on WebSocket connect:
  - If an entry already exists for the user, run displacement (send `SessionDisplaced`, await cleanup, force close if needed, trigger intercept cleanup)
  - Store the new connection metadata in `DevboxRegistry`
  - Emit telemetry/logging to indicate current environment selection (if any)
- [ ] Update intercept registration flow to refresh stored environment metadata (for status/UX) while relying on the single-session invariant (no additional environment-level displacement required)
- [ ] Extend `TunnelRegistry` for devbox tunnels
- [ ] Implement session cleanup logic:
  - On WebSocket disconnect or manual disconnect
  - If the connection owned any environments, trigger cleanup (unregister intercepts, free ownership)
  - Remove the session entry from `DevboxRegistry`
- [ ] Add heartbeat checker background task:
  - Check `DevboxRegistry` for connections with no recent activity (>90s)
  - Trigger cleanup for stale connections
- [ ] Add session and environment management REST endpoints:
  - `GET /v1/devbox/sessions` (list all user's login sessions from `devbox_session` table)
  - `GET /v1/devbox/connections` (list active devbox sessions from `DevboxRegistry`)
  - `GET /v1/devbox/environments` (list user's accessible environments for UI dropdown)
  - `GET /v1/devbox/active-environment` (get user's active environment from `devbox_session.active_environment_id`)
  - `PUT /v1/devbox/active-environment` (set active environment, body: `{environment_id: UUID}`)
  - `DELETE /v1/devbox/sessions/{id}` (revoke login session, disconnect all environments)
  - `DELETE /v1/devbox/connections/{user}` (disconnect the active devbox session for a user)

**6. Devbox CLI** (`crates/devbox/` or `lapdev-devbox/`)
- [ ] Create new Rust binary project for CLI
- [ ] Add Clap command structure:
  - `lapdev devbox login`
  - `lapdev devbox logout`
  - `lapdev devbox whoami`
  - `lapdev devbox sessions list/revoke`
  - `lapdev devbox connect`
  - `lapdev devbox disconnect`
  - `lapdev devbox status`
  - `lapdev devbox intercept list/stop` (management; creation happens in web UI)
  - `lapdev devbox tunnel` (optional direct tunneling)
- [ ] Implement OAuth device flow client:
  - Request device code
  - Display user code
  - Open browser
  - Poll for token
- [ ] Implement OS keychain integration (macOS/Linux/Windows)
- [ ] Implement devbox session flow:
  - Query API for user's active environment (`GET /v1/devbox/active-environment`) for display purposes
  - Establish WebSocket connections regardless of whether an environment is currently selected
  - Observe environment-change notifications (polling or push) so status output stays up to date
  - Automatic displacement detection handled when intercepts are registered
  - Store connection state in `~/.lapdev/session.json` (includes last-known environment if present)
- [ ] Implement service interception flow:
  - Receive `StartIntercept` instruction from API (triggered by web UI) and validate session state
  - Prerequisites check (must be connected to environment)
  - RPC call to register intercept within current environment (send service name, target port, chosen local port, devbox session id, short-lived tunnel token)
  - Local TCP listener
  - Bidirectional proxying
  - Graceful cleanup
  - Surface status updates back to UI (acknowledgements, errors)
- [ ] Add signal handlers (Ctrl+C) for cleanup
- [ ] Handle `SessionDisplaced` message:
  - Display notification with device name (and environment if provided)
  - Trigger graceful shutdown
  - Clean up all local resources (stop intercepts, close tunnels)
- [ ] Add optional flags for `connect`:
  - `--no-displace` — Fail if a devbox session is already active instead of displacing
 - `--force` — Immediate displacement without grace period
- [ ] Display old session info on displacement:
  - Show device name (and environment, if available) being displaced
  - Show "gracefully terminated" confirmation
- [ ] Session config file (`~/.lapdev/config.toml`)
- [ ] Session metadata cache (`~/.lapdev/session.json`) with current environment

**7. Web UI** (`lapdev-conductor/` or frontend)
- [ ] Device authorization page (`/device`)
  - User code entry form
  - SSO provider selection (GitHub/GitLab)
  - Scope selection UI (clusters, namespaces)
  - Approval confirmation
- [ ] Devbox environment selection page (`/devbox` or settings):
  - **Environment Selector**: Dropdown or list showing user's available environments
  - **Active Environment Indicator**: Clearly show which environment is currently active
  - **Set Active Button**: Update `devbox_session.active_environment_id` via API
  - Real-time status: show if environment has active connection (from `DevboxRegistry`)
  - Display session info: device name, connected since, active intercepts count
- [ ] Devbox intercept management UI (panel within `/devbox`):
  - Start intercept workflow: select service, namespace (if needed), local port, and target connected device
  - Validate device connectivity status before enabling submission; surface port binding errors returned by CLI
  - Show active intercept list with service name, local port, traffic stats, and stop buttons (including "Stop All")
  - Display empty state guidance when no devices are connected or no intercepts exist
  - Phase 2 controls: header-based routing fields (header name/value) for collaborative routing workflows
- [ ] Session management page (`/devbox/sessions`):
  - List all login sessions with device names, created/expires dates
  - Revoke session button
  - Show the active devbox session (device name, connected since)
- [ ] API endpoints for UI:
  - `GET /v1/devbox/environments` — list user's accessible environments
  - `GET /v1/devbox/active-environment` — get current active environment
  - `PUT /v1/devbox/active-environment` — set active environment (body: `{environment_id: UUID}`)
  - `GET /v1/devbox/connections` — list active devbox sessions from `DevboxRegistry`

**8. Tunnel Message Extensions** (`lapdev-common/`)
- [ ] Add `ServerTunnelMessage::OpenInterceptConnection` variant
  - Fields: `tunnel_id`, `service_name`, `namespace`, `source_addr`, `target_port`, `intercept_id`
- [ ] Add `ServerTunnelMessage::SessionDisplaced` variant
  - Fields: `reason`, `new_session_id`, `new_device_name`
- [ ] Ensure backward compatibility with existing preview URL tunnels

**9. Documentation**
- [ ] User guide for `lapdev devbox` CLI
- [ ] OAuth device flow setup guide
- [ ] Service interception tutorial with examples
- [ ] Architecture documentation
- [ ] Security considerations and best practices
- [ ] Troubleshooting guide

**10. Testing**
- [ ] Unit tests for `ServiceInterceptor` methods
- [ ] Integration tests for devbox session lifecycle and intercept flow (end-to-end)
- [ ] WebSocket connection tests
- [ ] Session displacement tests:
  - Same user connects from different devices (automatic displacement at connect)
  - All intercepts cleaned up when prior session is displaced
  - Graceful shutdown notification delivery (device name, optional environment context)
  - `--no-displace` flag behavior (fail on conflict)
  - `--force` flag behavior (immediate displacement)
- [ ] Environment-switching tests:
  - User changes the active environment in the web UI while `lapdev devbox connect` remains running
  - Intercepts for the previous environment are cleaned up before new ones are started
  - Status output reflects the newly selected environment without requiring a reconnect
- [ ] Multi-service tests:
  - Multiple service intercepts within the selected environment share the same devbox session lifecycle
- [ ] Prerequisites tests:
  - UI intercept request fails if the CLI is not connected or no environment is selected
  - Clear error messages direct the user to run `lapdev devbox connect` and/or select an environment in the web UI
- [ ] Cleanup tests (disconnect and crash scenarios):
  - All intercepts cleaned up on disconnect
  - All intercepts cleaned up on WebSocket disconnect
  - Stale `DevboxTunnel` routes / ConfigMaps cleaned up
- [ ] Sidecar proxy → devbox tunnel tests:
  - Sidecar registers with `SidecarProxyManager`, requests `DevboxTunnel` connection, and streams bytes end-to-end through `TunnelRegistry`
  - Auth token rejection path (invalid/expired `devbox_intercept_token`) returns clear errors and forces cleanup
- [ ] Load testing for sidecar proxy intercept mode
- [ ] Race condition tests:
  - Simultaneous `lapdev devbox connect` attempts from different devices for the same user
  - Database unique constraint enforcement for single-session-per-user rule

**11. Metrics & Observability**
- [ ] Prometheus metrics for devbox sessions
- [ ] Metrics for active intercepts
- [ ] Tunnel traffic metrics (bytes, connections, errors)
- [ ] Audit events for security logging
- [ ] Grafana dashboard for devbox monitoring

### Missing Components from Current Codebase
1. **Sidecar proxy enhancements**: Update existing preview sidecar image to understand `DevboxTunnel` routes and devbox tokens.
2. **OAuth device flow**: New endpoints and web UI for device authorization
3. **Devbox CLI binary**: Entirely new project managing the devbox session lifecycle (`connect`/`disconnect`) and service interception
4. **Service interception logic**: New feature in kube-manager for modifying K8s Services
5. **DevboxRegistry (in-memory)**: New component in API server for tracking the active devbox session per user and enforcing session displacement
6. **Database tables**: Three new tables with migrations:
   - `device_authorization` (device flow state)
   - `devbox_session` (login sessions, 30-day lifetime)
   - `devbox_service_intercept` (active intercept state)
7. **Session displacement**: API logic that enforces "last session wins" per user at connection time and notifies the displaced session

## Open Questions
- Should web UI include OAuth session management (list/revoke devbox sessions), or is CLI-only sufficient for initial release?
  - **Recommendation**: CLI-only for Phase 1, web UI in Phase 2 for team management.
- During device flow approval, should scope selection be mandatory or optional (default to all clusters user has access to)?
  - **Recommendation**: Optional with smart defaults; require explicit selection only for restricted scopes.
- Do we need HTTP(S) aware proxying initially, or is raw TCP sufficient for all use cases?
  - **Answer**: Raw TCP sufficient for Phase 1; HTTP awareness adds value in Phase 2 for observability.
- Should devbox tunnels be scoped to a specific environment/workspace to prevent cross-environment access?
  - **Recommendation**: Yes, namespace-scoped by default; RBAC enforces permissions.
- How will kube-manager learn which devbox instance to target when initiating reverse tunnels?
  - **Answer**: API maintains `DevboxRegistry` mapping session_id to WebSocket; kube-manager queries via RPC.
- Should device code TTL be configurable per-org, or is a fixed 5-minute window acceptable?
  - **Recommendation**: Fixed 5-minute TTL for Phase 1; make configurable in Phase 3 for enterprise.
