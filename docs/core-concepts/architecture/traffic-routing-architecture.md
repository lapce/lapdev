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
* Runs unmodified - no changes needed to your application code

**Lapdev Sidecar Proxy**

* Injected into each pod in Lapdev environments
* Handles traffic interception when Devbox is active
* Routes traffic between local machine and cluster services
* Transparent to your application code

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
