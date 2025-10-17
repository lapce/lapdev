# Devbox

**Devbox** is Lapdev’s local development companion — a CLI tool that connects your local machine directly to your Lapdev Kubernetes environment.

It bridges the gap between _local iteration speed_ and _production realism_.

### Why Devbox

Developing in Kubernetes usually means slow feedback loops: rebuild, redeploy, and wait for pods to restart just to test a single change.\
Devbox changes that by letting you **run your code locally while still connected to your real cluster environment**.

This means:

* You can test your app against live in-cluster services (databases, APIs, caches).
* Cluster traffic can be routed to your local process for real-time debugging.
* You no longer need complex port-forwarding, VPNs, or separate mock setups.

### How It Works

When you start Devbox inside a Lapdev environment:

1. It authenticates with Lapdev and connects to your active environment’s namespace.
2. It synchronizes local and cluster networking rules.
3. It can optionally intercept service traffic and forward it to your local process.
4. It provides seamless access to other workloads and in-cluster dependencies.

> 💡 Devbox doesn't replace Kubernetes — it _extends_ it for developers.\
> You keep your production topology and cluster configuration, but develop with local speed.

### Core Capabilities

* **Intercept Service Traffic:** Redirect in-cluster service requests to your local code.
* **In-Cluster Connectivity:** Access internal APIs and databases as if you were inside the pod.
* **Seamless IDE Debugging:** Run locally, attach debuggers, and see live logs.
* **Compatible with All Environment Types:** Works with personal, shared, and branch environments.

### How It Fits in the Lapdev Model

| Concept         | Role                                                                   |
| --------------- | ---------------------------------------------------------------------- |
| **App Catalog** | Defines what your app consists of (the workloads).                     |
| **Environment** | A running instance of that app in Kubernetes.                          |
| **Devbox**      | Bridges your local machine with that environment for live development. |

### When to Use Devbox

* When you need fast feedback without redeploying to Kubernetes.
* When debugging complex issues that depend on real cluster state.
* When integrating or testing locally while keeping the rest of the system in-cluster.

### Next Steps

Ready to use Devbox? See [**Local Development with Devbox**](../how-to-guides/local-development-with-devbox.md) for setup instructions.

