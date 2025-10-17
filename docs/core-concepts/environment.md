# Environment

The **Lapdev Kubernetes Environment** is the foundation of Lapdev’s platform. It’s where you **develop**, **test**, and **demo** your applications — all directly within Kubernetes.

Lapdev’s unique strength is that it manages the **entire environment lifecycle** — from creation to synchronisation — in a seamless, automated way. You don’t need to manually manage YAML files, sync resources, or worry about configuration drift.

### How It Works

Lapdev reads your **production Kubernetes manifests** directly from your cluster and treats them as the **single source of truth**.

To define your app environment:

1. Select the workloads you want to include.
2. Lapdev automatically finds and includes all related **ConfigMaps**, **Secrets**, and **Services**.
3. Lapdev replicates these resources into your Lapdev-managed environments.

The result: every developer’s environment mirrors production exactly — with zero manual setup. When your production apps change, Lapdev automatically syncs updates to all environments, so **everyone stays in sync**.

### Environment Models

Lapdev supports two environment models, depending on your workflow and cost requirements:

* **Personal Environments** — fully isolated, ideal for independent development
* **Branch Environments** — lightweight and cost-efficient, ideal for large teams

Both support **Devbox** for local development and debugging.

#### 1. Personal Environments

A **Personal Environment** is a completely isolated workspace for a single developer.

Each personal environment runs in its **own Kubernetes namespace**, so:

* One developer’s work never affects another’s.
* Multiple environments can be created per developer (e.g., one per feature or bug fix).
* You can switch between environments freely.

Because every personal environment contains a **full set of workloads**, it guarantees total isolation — but that comes with higher resource usage.

Lapdev helps mitigate this cost by allowing you to **start and stop environments on demand**, automatically scaling down resources when you’re not using them.

**Best for:**\
Developers who need full isolation or want to test complex changes safely.

#### 2. Branch Environments

To reduce cost while keeping flexibility, Lapdev introduces **Branch Environments**.

**Shared Environment Foundation**

Branch environments are built on top of **shared environments**, which are created and managed by admins. A shared environment runs a complete version of your app that everyone on the team can access.

When a developer creates a **branch environment**, it behaves much like a Git branch:

* Initially, the branch environment **does not create new Kubernetes resources**.
* It simply references the workloads in the shared environment.
* Only when you modify a workload does Lapdev create a **branched copy** of that specific workload to run side by side with the original.

This means creating new branch environments is almost instant — and nearly free in terms of compute cost.

**Local Development with Devbox**

Devbox allows you to:

* **Intercept cluster traffic** to your local machine for real-time debugging.
* Use your **local IDE** to edit, build, and debug your service as if it were running in the cluster.
* **Access in-cluster resources** such as databases, caches, and internal APIs transparently — no VPN or tunneling setup required.

When connected, Devbox runs your local process as part of the branch environment. Other services in the cluster automatically route traffic to your local instance, so you can develop locally while staying fully connected to the live environment.

This workflow combines the best of both worlds:

* Fast local feedback
* Accurate in-cluster integration

**Traffic Routing**

Under the hood, Lapdev uses a **sidecar proxy** within each workload to handle routing.\
This proxy decides whether incoming requests should go to:

* The **shared workload** (default),
* A **branched workload** created by a developer, or
* A **local Devbox process** connected to the environment.

Routing is based on the `tracestate` HTTP header, which identifies which branch a request belongs to.\
This enables multiple developers to safely test changes in the same shared environment — without interfering with each other.

For a deeper technical explanation of the sidecar proxy, routing algorithm, and how Lapdev ensures request isolation across branches, see the [**Traffic Routing Architecture**](architecture/traffic-routing-architecture.md) and [**Branch Environment Architecture**](architecture/branch-environment-architecture.md) page.

**Limitations**

Because branch environment's routing relies on the `tracestate` header:

* Your services must propagate the header through all internal HTTP calls.
* Non-HTTP components (like databases or message queues) can’t be routed and remain shared across branches.

Despite these limitations, branch environments provide an extremely efficient and collaborative workflow for large teams — combining shared stability with flexible, isolated development.

### Preview URLs

Every Lapdev environment can expose one or more **Preview URLs** — public HTTPS endpoints for accessing or sharing your running services.

Preview URLs are created **per service**. You choose which service to expose, and Lapdev automatically handles DNS, certificates, routing, and security.

When requests come in through a Preview URL in a branch environment, Lapdev automatically manages the **`tracestate` header** to route traffic to the correct branch environment.

You can read more about [preview URLs here](broken-reference).

### Comparison

| Feature         | Personal Environment                  | Branch Environment              |
| --------------- | ------------------------------------- | ------------------------------- |
| **Isolation**   | Fully isolated (per namespace)        | Shared base, routed isolation   |
| **Cost**        | Higher                                | Lower                           |
| **Ideal For**   | Deep testing, multi-service debugging | Large teams, frequent branching |
| **Performance** | Slower to start, full workloads       | Instant setup, minimal overhead |

### Summary

Lapdev Kubernetes Environments let you:

* Reproduce production exactly, without manual setup
* Keep all environments automatically synced with production
* Choose between **full isolation** or **lightweight branching**
* Use **Devbox** for fast, local-style development in Kubernetes
* Collaborate efficiently with consistent, shareable environments

Whether you’re developing alone or across a large team, Lapdev ensures every environment stays consistent, efficient, and production-accurate.
