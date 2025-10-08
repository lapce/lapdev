# Devbox Session & Workload Intercept Plan

> **Status**: Draft — covers backend, dashboard UI, and CLI/agent connect flows. Sidecar orchestration is referenced but specified elsewhere.

## 1. Goals

- Persist long-lived devbox sessions so users can reconnect without reconfiguration.
- Allow the dashboard to start/stop workload intercepts without duplicating service-level logic.
- Expose functionality exclusively via HRPC so both WASM (dashboard) and future native clients share one surface.
- Keep state minimal: DB tables should contain only the information required to rehydrate sidecar configuration and UX.

Out-of-scope for this revision: tunnel framing/data-plane protocol details (see devbox_tunnel_plan) and sidecar implementation specifics.

## 2. Data Model

### kube_devbox_session

| Field | Type | Notes |
|-------|------|-------|
| `id` | UUID PK | Generated at creation |
| `user_id` | UUID FK `users` | Session owner |
| `oauth_connection_id` | UUID FK | Source identity used during login |
| `session_token_hash` | TEXT | SHA-256 hash of PASETO token |
| `token_prefix` | TEXT | First 12 characters of token for support tooling |
| `device_name` | TEXT | Friendly label displayed in UI |
| `active_environment_id` | UUID nullable FK `kube_environment` | Web UI selection |
| `created_at` | TIMESTAMPTZ | Default `now()` |
| `expires_at` | TIMESTAMPTZ | Mirrors token expiry (30 days) |
| `last_used_at` | TIMESTAMPTZ | Updated on every authenticated request |
| `revoked_at` | TIMESTAMPTZ nullable | Marks soft deletion |

Constraints & indexes:
- Unique partial index on `(user_id)` where `revoked_at IS NULL` ensures one active session per user.
- Secondary indexes on `oauth_connection_id`, `active_environment_id`, `expires_at`, and `session_token_hash` for lookups.

### kube_devbox_workload_intercept

| Field | Type | Notes |
|-------|------|-------|
| `id` | UUID PK | Generated on creation |
| `session_id` | UUID FK `kube_devbox_session` | Owning session |
| `user_id` | UUID FK `users` | For auditing and multi-tenant queries |
| `environment_id` | UUID FK `kube_environment` | Namespace context |
| `workload_id` | UUID FK `kube_environment_workload` | Target workload |
| `port_mappings` | JSONB | Array of `{ "workload_port": u16, "local_port": u16, "protocol": "TCP" }` |
| `created_at` | TIMESTAMPTZ | Default `now()` |
| `restored_at` | TIMESTAMPTZ nullable | Marks completion/cleanup |

Indexes:
- `idx_workload_intercept_session` on `session_id`.
- `idx_workload_intercept_environment` on `environment_id`.
- `idx_workload_intercept_workload` on `workload_id`.
- `idx_workload_intercept_active` partial on `restored_at IS NULL`.
- `idx_workload_intercept_user_env` on `(user_id, environment_id)`.
- `idx_workload_intercept_unique_active` partial on `(user_id, workload_id)` where `restored_at IS NULL` to forbid multiple simultaneous intercepts per workload.

No ConfigMap snapshots or routing keys are persisted; reconciliation depends on reapplying `port_mappings` for a workload when needed.

## 3. HRPC API Surface

### Session services

| Method | Request | Response | Notes |
|--------|---------|----------|-------|
| `DevboxSessionService.ListSessions` | `{}` | `{ sessions: Vec<SessionSummary> }` | Includes active + historical sessions |
| `DevboxSessionService.RevokeSession` | `{ session_id }` | `{}` | Soft deletes session and cleans up dependent intercepts |
| `DevboxSessionService.GetActiveEnvironment` | `{}` | `{ environment_id, cluster_name, namespace }` | Returns selection for the caller’s session |
| `DevboxSessionService.SetActiveEnvironment` | `{ environment_id }` | `{}` | Updates session record + pushes websocket notification |
| `DevboxSessionService.WhoAmI` | `{}` | `{ user_id, email, device_name, authenticated_at, expires_at }` | Convenience for status banner |

Authentication: Bearer token (PASETO) identical to existing API surface. The HRPC layer runs inside API server.

### Workload intercept services

| Method | Request | Response | Notes |
|--------|---------|----------|-------|
| `DevboxInterceptService.ListWorkloadIntercepts` | `{ environment_id }` | `{ intercepts: Vec<WorkloadInterceptSummary> }` | Active + historical |
| `DevboxInterceptService.StartWorkloadIntercept` | `{ workload_id, port_mappings: Vec<PortMappingOverride> }` | `{ intercept_id }` | Creates/updates intercept |
| `DevboxInterceptService.StopWorkloadIntercept` | `{ intercept_id }` | `{}` | Marks `restored_at` and triggers cleanup |

`PortMappingOverride` omits unspecified ports; server fills defaults from workload spec. HRPC handlers validate environment ownership and write to DB, then notify the sidecar orchestration layer (future document).

## 4. Dashboard UI Flow

1. **Environment selection** – The header dropdown calls `SetActiveEnvironment`; response updates local state and server pushes websocket event.
2. **Workload grid** – Fetch via existing environment detail API, augment with `ListWorkloadIntercepts` to display which workloads are intercepted (badge with local ports + device).
3. **Start intercept** – Clicking “Intercept” on a workload opens modal listing container ports with editable local values. Submit through `StartWorkloadIntercept` HRPC. On success close modal, refresh `ListWorkloadIntercepts`, show toast.
4. **Stop intercept** – “Stop” button on the workload badge calls `StopWorkloadIntercept`; optimistic update, revert on failure.
5. **Auto-reconnect** – When websocket reconnects, call `ListWorkloadIntercepts` to ensure UI reflects migrated intercepts and display a toast “Reattached intercepts from previous session”.
6. **History tab (optional)** – Secondary view surfaces archived intercepts (restored state, timestamps, user/device) using the same list call with `include_restored = true` parameter (to be added if required).

## 5. CLI Devbox Connect Flow

CLI responsibilities focus on establishing and maintaining the devbox session over WebSockets (control plane) plus the data-plane tunnels used for port forwarding. HRPC is not available to the CLI (non-WASM), so it reuses REST for login/token retrieval but relies on WebSockets for session orchestration.

### 5.1 Prerequisites

1. `lapdev devbox login` (existing flow) stores the PASETO token in the OS keychain.
2. CLI reads `LAPDEV_API_URL` (default `https://app.lap.dev`).
3. CLI loads token from keychain before attempting to connect; exits with guidance if absent.

### 5.2 Control-plane WebSocket (`/kube/devbox/tunnel`)

Steps executed by `lapdev devbox connect`:

1. Load token from keychain; build `Authorization: Bearer <token>` header.
2. Open WebSocket to API endpoint `/kube/devbox/tunnel` (TLS) with headers:
   - `Authorization`
   - `X-Lapdev-Client-Version`
   - `X-Lapdev-Platform` (optional for telemetry)
3. API authenticates using the logic defined in this doc (session lookup, intercept migration, replay). On success it sends a `ServerHello` message containing:
   - `session_id`
   - `user_id`
   - `device_name`
   - `port_mappings` for any active intercepts already migrated
4. CLI responds with `ClientHelloAck` confirming readiness.
5. API replays `StartIntercept` messages (one per workload) if any intercepts exist; CLI must set up local listeners per mapping.

Control-plane message handling on the CLI:
- `StartIntercept { intercept_id, workload_name, port_mappings }`
  - For each mapping ensure a local TCP listener exists; if already bound, reuse.
  - Start a data-plane tunnel (see below) for each mapping.
  - Reply with `StartInterceptAck { intercept_id }` once listeners/tunnels are ready.
- `StopIntercept { intercept_id }`
  - Tear down associated data-plane tunnels and close local listeners created for this intercept.
  - Reply with `StopInterceptAck` and remove state.
- `SessionDisplaced { new_device_name }`
  - Gracefully shut down listeners/tunnels and exit with a notice (old device takes precedence).
- Heartbeats (`Ping`/`Pong`) – CLI must respond within 5s to keep session alive.

### 5.3 Data-plane tunnels

For each port mapping in a `StartIntercept` message:

1. CLI creates (or reuses) a local TCP listener on the requested `local_port`.
2. Upon first inbound connection, CLI opens a dedicated WebSocket to `/kube/devbox/tunnel/data` providing:
   - `Authorization` header with session token.
   - Query parameters or JSON handshake containing `intercept_id`, `workload_port`, and `local_port`.
3. API associates the data-plane socket with the intercept and begins forwarding frames to the connected devbox (future agent design outside this doc).
4. CLI streams bytes bidirectionally between the local socket and the WebSocket.
5. When the local connection closes, CLI sends `CloseDataTunnel` frame and keeps the listener alive for future connections until `StopIntercept` arrives.

### 5.4 Reconnect & resume

- If the control-plane WebSocket drops (network blip), CLI should retry with exponential backoff (starting at 1s, capped at 30s).
- Upon reconnect, API will re-run the session migration logic and replay `StartIntercept` messages. CLI must rehydrate listeners/tunnels automatically.
- If CLI cannot bind a requested local port (already in use), it should prompt the user and send `StartInterceptError` back to API. API can then surface the failure in the dashboard.

### 5.5 Disconnect behaviour

- `Ctrl+C` or SIGINT triggers graceful shutdown: CLI sends `ClientDisconnect`, closes all data tunnels, removes local listeners, and exits with code 0.
- If CLI exits unexpectedly, API’s heartbeat timeout/`DevboxRegistry` cleanup will mark the connection stale and stop intercepts.

## 6. Background Jobs & Event Handling

- `cleanup_stale_kube_devbox_sessions`: mark sessions revoked if `expires_at < now` or no activity longer than threshold; ensure dependent intercepts are stopped.
- `cleanup_stale_workload_intercepts`: detect intercepts left active when workloads/environments disappear; set `restored_at` and notify orchestration layer.
- Websocket registry: when a new session displaces an old one, migrate intercept rows to the new `session_id`, replay `StartWorkloadIntercept` messages with existing `port_mappings`, and render UI toast (handled through HRPC event channel / websocket payloads).

## 7. Implementation Checklist

1. **Database**
   - [ ] Create `kube_devbox_session` migration (unique partial on active session, indexes).
   - [ ] Create `kube_devbox_workload_intercept` migration with `port_mappings` JSONB and unique `(user_id, workload_id)` partial index.
   - [ ] Generate SeaORM entities and update `Migrator` wiring.

2. **Session services**
   - [ ] Adapt `session_authorize` flow: create new session row, migrate active intercepts to the new session ID, revoke old session.
   - [ ] Implement HRPC handlers in `DevboxSessionService` (list, revoke, get/set active environment, whoami).
   - [ ] Migrate any remaining REST callers in dashboard to HRPC and deprecate unused REST endpoints.
   - [ ] Ensure websocket registry displacement sends replay events for migrated intercepts.

3. **Intercept services**
   - [ ] Implement `StartWorkloadIntercept` HRPC handler: validate ownership, upsert intercept, update DB, trigger orchestration.
   - [ ] Implement `StopWorkloadIntercept` handler: mark `restored_at`, trigger orchestration cleanup.
   - [ ] Implement `ListWorkloadIntercepts` handler: join workloads for display metadata.
   - [ ] Update dashboard to consume HRPC methods (Leptos queries/mutations).

4. **Background and telemetry**
   - [ ] Add session cleanup jobs (stale + expired) that also stop intercepts.
   - [ ] Add intercept cleanup job to detect orphaned rows.
   - [ ] Emit structured logs/metrics for intercept lifecycle events.

5. **CLI agent**
   - [ ] Implement `lapdev devbox connect` command: load token, open control-plane WebSocket, handle handshakes/messages.
   - [ ] Implement local port listeners + data-plane WebSocket creation per `StartIntercept` instruction.
   - [ ] Handle reconnection (exponential backoff), displacement, and graceful shutdown (Ctrl+C).
   - [ ] Surface user-friendly errors when local ports are occupied or tunnel setup fails.

## 8. Success Criteria

1. Single active session per user enforced by DB + HRPC.
2. Session migration (login on new device) preserves active workload intercepts without manual UI input.
3. Dashboard uses only HRPC calls for session/intercept operations.
4. Intercept creation stores workload-level port mappings and replays them after reconnection.
5. Unique `(user_id, workload_id)` constraint prevents double intercepts; attempting to start a second intercept returns informative error.
6. Stale sessions/intercepts are removed automatically by background jobs.
7. Historical data remains queryable (`restored_at` timestamps).
8. `lapdev devbox connect` establishes the control-plane WebSocket, responds to replayed intercepts, and emits acknowledgements.
9. CLI spins up data-plane tunnels according to `port_mappings`, reattaches after reconnect, and reports binding failures gracefully.

## 9. Open Questions

- Should `port_mappings` support protocols beyond TCP (e.g., UDP) or metadata like HTTP hostnames?
- How does the orchestration layer authenticate/authorize replayed intercepts once the client/agent protocol is redesigned?
- Do we need a dedicated HRPC stream for session/intercept events instead of piggybacking on the existing websocket?
- Are there administrative requirements (e.g., organization-wide session revocation) that warrant separate HRPC endpoints?
