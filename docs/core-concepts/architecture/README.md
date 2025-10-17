# Architecture



This document explains how Lapdev works and how its components interact to provide seamless Kubernetes development environments.

### Overview

Lapdev consists of three main components:

1. **Lapdev API Server** (SaaS) - Manages users, authentication, and orchestrates environment creation
2. **Lapdev-Kube-Manager** (In your cluster) - Reads production manifests and manages dev environments
3. **Devbox CLI** (Developer's machine) - Enables local debugging with cluster connectivity

### Architecture Diagram

<img src="../../.gitbook/assets/file.excalidraw.svg" alt="" class="gitbook-drawing">

### Component Details

#### Lapdev API Server (SaaS)

The Lapdev cloud service handles:

* **User authentication and authorization** - GitHub/Google OAuth, team management
* **Environment orchestration** - Receives environment creation requests from users
* **Secure tunnel management** - Establishes websocket tunnels between your cluster and Lapdev
* **Preview URL routing** - Routes traffic from preview URLs (e.g., `alice-feature.app.lap.dev`) to your cluster

**Security:**

* Communicates with your cluster via secure websocket tunnels (TLS encrypted)
* No direct access to your cluster's API server
* Cannot read your application data or secrets

#### Lapdev-Kube-Manager (In Your Cluster)

Deployed as a single Kubernetes deployment in your cluster, `lapdev-kube-manager`:

* **Reads production manifests** - Discovers Deployments, StatefulSets, ConfigMaps, Secrets, and Services from your production namespace
* **Creates dev environments** - Replicates selected workloads into isolated or shared namespaces
* **Manages sync** - Monitors production manifests for changes and updates dev environments
* **Handles traffic routing** - For branch environments, routes traffic to the correct version of services
* **Establishes secure tunnel** - Maintains websocket connection to Lapdev API Server for orchestration

**Permissions:**

* Read access to production namespace (to read manifests)
* Full access to Lapdev-managed namespaces (to create/update environments)
* No access to other cluster resources

#### Devbox CLI (Developer Machine)

The `devbox` command-line tool enables local development:

* **Traffic interception** - Routes requests for specific services to `localhost`
* **Cluster connectivity** - Provides transparent access to in-cluster services (databases, APIs, caches)
* **Secure tunnel** - Establishes encrypted connection to Lapdev API Server, which proxies to your cluster

**How it works:**

1. Developer runs `lapdev devbox intercept checkout-service`
2. Devbox establishes secure tunnel: `Developer → Lapdev API → lapdev-kube-manager`
3. Traffic for `checkout-service` is routed to developer's localhost
4. Developer's code can access cluster services transparently (e.g., `http://payment-service:8080`)
