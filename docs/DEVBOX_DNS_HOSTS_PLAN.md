# Devbox Hosts-Based DNS Plan

## Goals
- Support short-name access to Kubernetes services from the devbox CLI without running a full DNS proxy.
- Reuse the existing WebSocket tunnel (`/kube/devbox/tunnel/client/{session}`) for data-plane traffic.
- Keep OS networking changes minimal and reversible while covering macOS, Linux, and Windows.

## High-Level Approach
1. **Service Discovery**
   - When the CLI connects and learns the active environment, fetch the list of services/ports for that environment (via API/RPC).
   - Refresh this list whenever the active environment changes; optionally poll periodically to catch new services.

2. **Synthetic Loopback Allocation**
   - Reserve a loopback subnet (e.g., `127.77.0.0/24`).
   - Assign one IP per `(service, port)` pair and track the mapping in memory.
   - Reuse IPs if a service/port already exists; recycle IPs when entries disappear.

3. **Hosts File Injection**
   - Generate aliases per service (e.g., `service`, `service.namespace`, `service.namespace.svc`, `service.namespace.svc.cluster.local`).
   - Write a bounded block of entries pointing to the synthetic IPs between markers, e.g.:
     ```
     # BEGIN lapdev-devbox
     127.77.0.5 service service.namespace service.namespace.svc service.namespace.svc.cluster.local
     # END lapdev-devbox
     ```
   - Implement per-OS writers:
     - macOS/Linux: `/etc/hosts` (requires sudo).
     - Windows: `C:\Windows\System32\drivers\etc\hosts` (requires admin; ensure CRLF endings).
   - If we cannot modify the file (permissions, policy), emit explicit manual instructions instead of failing silently.

4. **Connection Bridge**
   - Run a `TcpListener` covering the synthetic block (bind to each assigned IP or use reusable listeners).
   - On incoming connections:
     1. Look up the destination IP/port to find the target service.
     2. Use `TunnelClient::connect_tcp(fqdn, port)` to open a tunneled connection to `service.namespace.svc.cluster.local`.
     3. Proxy bytes bidirectionally between the local socket and the tunnel stream.
   - Close connections gracefully and recycle IPs on idle/timeout.

5. **Lifecycle Handling**
   - **On connect**: start tunnel tasks, fetch services, write hosts block, start listener.
   - **Environment change**: remove existing hosts block, refresh services, rebuild mappings, rewrite hosts, and restart listener if needed.
   - **Shutdown (Ctrl+C or displacement)**: stop listener, remove hosts block, drop tunnel client.

6. **Safety & Observability**
   - Always guard host-file edits with clear markers so we never disturb user-managed entries.
   - Log every change (hosts written, listener started, tunnel connection opened).
   - Provide dry-run / verbose modes for debugging.

7. **Cross-Platform Notes**
   - macOS: prompt for sudo; use `/etc/hosts`.
   - Linux: same as macOS; avoid touching NetworkManager-managed files.
   - Windows: require elevated PowerShell or document manual steps.

8. **Validation Plan**
   - Unit tests for hosts block rendering, synthetic IP allocator, and mapping lookups.
   - Integration test: mock tunnel server, apply hosts updates to a temp file, verify `curl http://service` routes through the synthetic listener.
   - Manual smoke tests on macOS/Linux/Windows to confirm hosts entries are written/removed and that short names resolve through the tunnel.

This plan keeps DNS changes simple (hosts file only) while leveraging the existing tunnel to reach cluster services under short names for the active environment.
