# Lapdev RPC Architecture

Lapdev combines tarpc-based transports with HTTP RPC (HRPC) endpoints to coordinate the control plane, workspace hosts, developers' CLIs, and Kubernetes agents. The diagram below highlights the main RPC links and which component resides on each side of the connection.

## RPC Topology

```mermaid
flowchart LR
    subgraph Clients
        CLI["Devbox CLI"]
        Dashboard["Dashboard / Browser"]
    end

    subgraph API["Lapdev API (CoreState)"]
        APIHttp["HTTP router & HRPC endpoints"]
        DevboxRPC["Devbox RPC server"]
        KubeCtl["Kube controller"]
        Tunnel["Tunnel broker"]
    end

    subgraph Kubernetes["Kubernetes Agents"]
        KM["lapdev-kube-manager"]
        Sidecar["kube-sidecar-proxy"]
        DevboxProxy["kube-devbox-proxy"]
    end

    Dashboard -- "HTTPS (HRPC)<br/>HrpcService" --> APIHttp
    CLI <-->|"WebSocket + tarpc<br/>DevboxSessionRpc & DevboxClientRpc"| DevboxRPC
    CLI <-->|"WebSocket tunnels<br/>tunnel crate"| Tunnel
    KubeCtl <-->|"WebSocket + tarpc<br/>KubeClusterRpc & KubeManagerRpc"| KM
    KM <-->|"TCP + tarpc<br/>SidecarProxyManagerRpc & SidecarProxyRpc"| Sidecar
    KM <-->|"TCP + tarpc<br/>DevboxProxyManagerRpc & DevboxProxyRpc"| DevboxProxy
    Sidecar <-->|"WebSocket tunnels<br/>Preview & intercept data"| Tunnel
    DevboxProxy <-->|"WebSocket tunnels<br/>Branch env traffic"| Tunnel
    linkStyle 0 stroke:#64748b,stroke-width:2px,stroke-dasharray:6 3
    linkStyle 1 stroke:#64748b,stroke-width:2px,stroke-dasharray:6 3
    linkStyle 2 stroke:#2ca58d,stroke-width:3px
    linkStyle 3 stroke:#64748b,stroke-width:2px,stroke-dasharray:6 3
    linkStyle 4 stroke:#64748b,stroke-width:2px,stroke-dasharray:6 3
    linkStyle 5 stroke:#64748b,stroke-width:2px,stroke-dasharray:6 3
    linkStyle 6 stroke:#2ca58d,stroke-width:3px
    linkStyle 7 stroke:#2ca58d,stroke-width:3px
    classDef client fill:#fef3c7,stroke:#f59e0b,stroke-width:2px,color:#78350f;
    classDef api fill:#e0f2fe,stroke:#0ea5e9,stroke-width:2px,color:#0f172a;
    classDef kube fill:#dcfce7,stroke:#22c55e,stroke-width:2px,color:#14532d;
    classDef tunnelNode fill:#fef9c3,stroke:#facc15,stroke-width:2px,color:#713f12;
    class CLI,Dashboard client
    class APIHttp,DevboxRPC,KubeCtl api
    class KM,Sidecar,DevboxProxy kube
    class Tunnel tunnelNode
```

Green edges represent high-throughput data-plane tunnels; dashed gray edges denote tarpc/HRPC control-plane calls.

## Link details

| Source → Target | Transport | RPC traits / channel | Purpose |
| --- | --- | --- | --- |
| Dashboard → API HTTP router | HTTPS (JSON) | `HrpcService` (lapdev_hrpc) | Dashboard and admin UI call HRPC endpoints for user, org, and Kubernetes environment management |
| Devbox CLI ↔ Devbox RPC server | WebSocket + tarpc | `DevboxSessionRpc`, `DevboxClientRpc` | CLI maintains a control-plane session for authentication, intercept lifecycle, and device displacement notifications |
| Devbox CLI ↔ Tunnel broker | WebSocket (tunnel crate) | Multiplexed TCP streams | Streams intercepted workload traffic from the cluster back to developer machines |
| Kube controller ↔ lapdev-kube-manager | WebSocket + tarpc | `KubeClusterRpc`, `KubeManagerRpc` | Control plane for Kubernetes workloads: cluster registration, workload discovery, deployments, and tunnel heartbeats |
| lapdev-kube-manager ↔ kube-sidecar-proxy | TCP + tarpc | `SidecarProxyManagerRpc`, `SidecarProxyRpc` | Configures per-workload sidecar proxies, distributes devbox intercept routes, and collects proxy heartbeats/metrics |
| lapdev-kube-manager ↔ kube-devbox-proxy | TCP + tarpc | `DevboxProxyManagerRpc`, `DevboxProxyRpc` | Registers environment-scoped devbox proxies and pushes branch-environment routing updates |
| kube-sidecar-proxy ↔ Tunnel broker | WebSocket (tunnel crate) | Multiplexed TCP streams | Data-plane path for preview URLs and live intercept traffic between in-cluster workloads and the API |
| kube-devbox-proxy ↔ Tunnel broker | WebSocket (tunnel crate) | Multiplexed TCP streams | Data-plane path for branch environments that proxy traffic back through the API |

## Notes

- The tunnel broker, Devbox RPC server, and kube controller all run inside the Lapdev API process, but they expose distinct RPC surfaces to external agents.
- All tarpc links use the shared `spawn_twoway` helper to provide simultaneous client and server channels over a single transport.
- HRPC endpoints are consumed both by the dashboard and by service-to-service calls that need simple HTTP semantics (for example, branch environment orchestration).
- Sidecar and devbox proxies establish long-lived WebSocket tunnels only after control-plane authorization succeeds via their respective tarpc channels.
