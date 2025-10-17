# Traffic Routing Architecture

This document explains how traffic flows through Lapdev for both preview URLs and local development with Devbox.

### Overview

Lapdev handles two main traffic patterns:

1. **Preview URL Traffic** - External users accessing your development environment via browser
2. **Devbox Traffic** - Developers debugging locally while accessing cluster services

Both patterns use secure tunnels through the Lapdev cloud service, eliminating the need for VPNs or firewall changes.

### Architecture Diagram

<img src="../../.gitbook/assets/file.excalidraw (2).svg" alt="" class="gitbook-drawing">

### Components

#### In Your Cluster

**Lapdev-Kube-Manager**

* Orchestrates environment creation and management
* Maintains connection to Lapdev cloud service

**Lapdev Environment (Namespace)**

* Contains your replicated workloads
* Each environment is isolated in its own namespace
* Multiple environments can coexist in the same cluster

**App Workload Pod**

* Your application container(s)
* Runs unmodified in Personal and Shared environments
* Branch environments may require header propagation (see Branch Environment Routing below)

**Lapdev Sidecar Proxy**

* Automatically injected into each pod in Lapdev environments
* Routes traffic for branch environments based on tracestate headers
* Handles traffic interception when Devbox is active
* Routes traffic between local machine and cluster services

#### In Lapdev Environment (Namespace-level)

**Lapdev Devbox Proxy**

* Deployed once per environment namespace
* Manages all Devbox intercept connections for that environment
* Routes traffic to the appropriate developer's local machine
* Falls back to in-cluster service if no intercept is active

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

1. **Browser** → Request to `https://service-name.app.lap.dev`
2. **Lapdev Cloud Service** → Authenticates request (if access control enabled)
3. **Lapdev Cloud Service** → Routes through WebSocket tunnel to kube-manager
4. **Kube-Manager** → Forwards to appropriate environment namespace
5. **Sidecar Proxy** → Routes to target service based on environment type:
   - **Personal/Shared:** Routes directly to service
   - **Branch:** Checks tracestate header and routes to branched or shared version
6. **Service** → Processes request and returns response

The response flows back through the same path to the user's browser.

#### Devbox Intercept Traffic

When a developer intercepts a service with Devbox:

1. **Developer runs** `lapdev devbox connect` and enables intercept in dashboard
2. **Devbox CLI** → Establishes secure tunnel: `Local machine → Lapdev Cloud → Kube-Manager → Devbox Proxy`
3. **Cluster traffic** for intercepted service → Routes to Devbox Proxy
4. **Devbox Proxy** → Forwards to developer's local machine via tunnel
5. **Local service** → Developer's code running on localhost processes the request
6. **Response** flows back through tunnel to cluster

When no intercept is active, traffic routes normally to the in-cluster service.

#### Branch Environment Routing

Branch environments use intelligent routing based on tracestate headers:

1. **Request enters** through Preview URL with branch-specific tracestate header (auto-injected by Lapdev)
2. **Sidecar Proxy** reads tracestate header to identify the branch
3. **Routing decision:**
   - Service modified in branch? → Route to branch version
   - Service not modified? → Route to shared environment version
4. **Header propagation:** Application must forward headers to downstream services
5. **Next hop:** Process repeats at each service

This enables multiple developers to test different modifications simultaneously without conflicts.

> **Important:** Branch environment routing requires your application to propagate the tracestate header in HTTP calls. See [Branch Environment Architecture](branch-environment-architecture.md) for implementation details.

### Component Roles Summary

| Component | Purpose | When Used |
|-----------|---------|-----------|
| **Sidecar Proxy** | Routes traffic based on environment type and headers | All environments |
| **Devbox Proxy** | Manages intercept tunnels to developer machines | When Devbox is active |
| **Kube-Manager** | Orchestrates environments and maintains cloud connection | Always running |
| **Lapdev Cloud** | Routes external traffic and manages authentication | Preview URLs and Devbox |

### Learn More

For detailed information on specific routing patterns:

* **Branch Environment Routing** - See [Branch Environment Architecture](branch-environment-architecture.md) for tracestate header propagation, routing mechanism, and troubleshooting
* **Devbox Integration** - See [Devbox](../devbox.md) for local development workflows
* **Cluster Architecture** - See [Architecture Overview](README.md) for overall system design
