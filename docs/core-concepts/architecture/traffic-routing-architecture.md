# Traffic Routing Architecture

This document explains how traffic flows through Lapdev for both preview URLs and local development with Devbox.

### Overview

Lapdev handles two main traffic patterns:

1. [**Preview URL**](../preview-url.md) **Traffic** - External users accessing your development environment via browser
2. [**Devbox**](../devbox.md) **Traffic** - Developers debugging locally while accessing cluster services

Both patterns use secure tunnels through the Lapdev cloud service, eliminating the need for VPNs or firewall changes.

### Architecture Diagram

<img src="../../.gitbook/assets/file.excalidraw (1).svg" alt="" class="gitbook-drawing">

### Components

#### In Your Cluster

**Lapdev-Kube-Manager**

* Orchestrates environment creation and management
* Maintains control-plane connection to Lapdev cloud service (route updates, heartbeats)
* Pushes branch/service routing tables and Devbox intercept metadata to sidecars

**Lapdev** [**Environment**](../environment.md) **(Namespace)**

* Contains your replicated workloads
* Each environment is isolated in its own namespace
* Multiple environments can coexist in the same cluster

**App Workload Pod**

* Your application container(s)
* Runs unmodified in Personal and Shared environments
* [Branch environments](branch-environment-architecture.md) may require header propagation (see Branch Environment Routing below)

**Lapdev Sidecar Proxy**

* Automatically injected into each pod in Lapdev environments
* Routes traffic for branch environments based on tracestate headers
* Handles Devbox intercepts directly (opens tunnels to Lapdev cloud and shuttles pod traffic over them)
* Falls back to in-cluster service when no intercept is active

#### External Components

**Lapdev Cloud Service**

* Routes preview URL traffic to your cluster
* Manages secure websocket tunnels
* Handles authentication for preview URLs

**Devbox (Developer Machine)**

* CLI tool running on developer's laptop
* Establishes secure tunnel to cluster
* Intercepts traffic for specific services
* Provides transparent access to in-cluster services

### Traffic Flows

#### Preview URL Traffic

When a user accesses a Preview URL:

1. **Browser** → Request to automatically generated HTTPS URL
2. **Lapdev Cloud Service** → Authenticates request (if access control enabled)
3. **Lapdev Cloud Service** → Routes through WebSocket tunnel to kube-manager
4. **Kube-Manager** → Forwards to appropriate environment namespace
5. **Sidecar Proxy** → Routes to target service based on environment type:
   * **Personal/Shared:** Routes directly to service
   * **Branch:** Checks tracestate header and routes to branched or shared version
6. **Service** → Processes request and returns response

The response flows back through the same path to the user's browser.

#### Devbox Intercept Traffic

When a developer intercepts a service with Devbox:

1. **Developer runs** `lapdev devbox connect` and enables intercept in dashboard
2. **Devbox CLI** → Establishes secure tunnel: `Local machine → Lapdev Cloud → Sidecar Proxy`
   * Kube-Manager stays on the control plane (publishing intercept metadata and optional direct-connect hints) but is **not** on the data path.
3. **Sidecar Proxy** for the intercepted pod:
   * Receives routing rules from Kube-Manager
   * Opens the tunnel to Lapdev Cloud using the intercept token
   * Forwards intercepted traffic to the developer’s local machine
4. **Local service** → Developer's code running on localhost processes the request
5. **Response** flows back through the same tunnel to the pod

When no intercept is active, traffic routes normally to the in-cluster service.

#### Branch Environment Routing

Branch environments use intelligent routing based on tracestate headers:

1. **Request enters** through Preview URL with branch-specific tracestate header (auto-injected by Lapdev)
2. **Sidecar Proxy** reads tracestate header to identify the branch
3. **Routing decision:**
   * Service modified in branch? → Route to branch version
   * Service not modified? → Route to shared environment version
4. **Header propagation:** Application must forward headers to downstream services
5. **Next hop:** Process repeats at each service

This enables multiple developers to test different modifications simultaneously without conflicts.

> **Important:** Branch environment routing requires your application to propagate the tracestate header in HTTP calls. See [Branch Environment Architecture](branch-environment-architecture.md) for implementation details.

### Component Roles Summary

| Component         | Purpose                                                                  | When Used               |
| ----------------- | ------------------------------------------------------------------------ | ----------------------- |
| **Sidecar Proxy** | Routes traffic based on environment type and headers                     | All environments        |
| **Kube-Manager**  | Orchestrates environments and pushes routing/intercept state to sidecars | Always running          |
| **Lapdev Cloud**  | Routes external traffic and manages authentication                       | Preview URLs and Devbox |

### Learn More

**Specialized Routing Documentation:**

* [Branch Environment Architecture](branch-environment-architecture.md) - Tracestate header propagation, routing mechanism, and troubleshooting
* [Architecture Overview](./) - Overall system design and component interactions

**Core Concepts:**

* [Environment](../environment.md) - Personal, Shared, and Branch environment types
* [Devbox](../devbox.md) - Local development with cluster connectivity
* [Preview URL](../preview-url.md) - HTTPS access to your services

**How-To Guides:**

* [Use Preview URLs](../../how-to-guides/use-preview-urls.md) - Create and manage preview URLs
* [Local Development with Devbox](../../how-to-guides/local-development-with-devbox.md) - Set up traffic interception and cluster access
* [Create Lapdev Environment](../../how-to-guides/create-lapdev-environment.md) - Set up different environment types
