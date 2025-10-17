# Introduction

Kubernetes makes deploying apps easy, well, sort of. But that 100 microservices app makes the development flow hard. You can't fit the whole app on your workstation, without pain. The most sensible way is probably running the development flow in Kubernetes itself. Have a pipeline to build and deploy your code changes. Ouch, the feedback is slow.&#x20;

Tools like Skaffold can sync your code changes to pods directly, and tools like Telepresence can intercept traffic to your local machine which enables you to do local debugging. That makes things feel like good old local development again. But.

**How do you manage all the Kubernetes resources?** The deployments, ConfigMaps, services, secrets - who maintains them? Do developers share the same resources, or does everyone get separate environments? If they're separate, how do you keep them in sync? How do you ensure changes to the app propagate to all dev environments? And what about domains, DNS, and HTTPS for developers to actually access their work?

## Why Lapdev

Lapdev Kubernetes Environment solves all these pains so that you don't have to.

### **Seamless Environment Management:**

Lapdev reads your **production Kubernetes manifests directly from your cluster**. Just tell Lapdev which workloads your app needs, and it automatically replicates:

* The workloads (Deployments, StatefulSets, DaemonSets, etc.)
* Associated ConfigMaps and Secrets
* Services and networking configuration

**Your production manifests become the single source of truth** - no duplicate YAML files to maintain, no config drift between prod and dev.

### Automatic Sync with Production

* Lapdev continuously monitors your production manifests for changes
* App Catalogs automatically update when their source workloads change
* All environments created from the catalog use the latest configuration
* **Never again:** "My dev environment is 3 months behind prod"

### Flexible Environment Models

**Personal Environments:** Each developer gets a complete, independent copy of all workloads. Perfect for testing breaking changes or complex multi-service interactions with full isolation.

**Branch Environments (Cost-Effective):** A shared baseline environment runs all services once. Developers create lightweight "branch environments" for only the services they're actively modifying. Lapdev automatically routes your traffic to your version while everything else uses the shared baseline.

### Local Development with Devbox

Lapdev includes **Devbox**, a CLI tool that integrates seamlessly with your environments:

* Intercept cluster traffic to your local machine for real-time debugging
* Use your local IDE, set breakpoints, and see live logs
* Transparently access in-cluster services (databases, caches, internal APIs) as if you're running inside the pod
* No complex VPN or tunneling setup required

### Preview URLs with Zero Configuration

Create Preview URLs for any service in your environment:

* Unique HTTPS URLs automatically generated for each service
* Automatic TLS certificates and DNS configuration
* Traffic proxied directly to your in-cluster services
* Optional access control - configure URLs to be accessible only to Lapdev logged-in users
* No firewall changes, no manual Ingress configuration, no cert-manager setup

Share your work with teammates, PMs, or QA instantly - they can test your changes without any cluster access or VPN.

### Easy Installation in Your Cluster

Lapdev requires just one deployment `lapdev-kube-manager` installed in your cluster. That's it.
