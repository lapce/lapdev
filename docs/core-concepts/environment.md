# Environment

The **Lapdev Kubernetes Environment** is the foundation of Lapdev’s platform. It’s where you **develop**, **test**, and **demo** your applications — all directly within Kubernetes.

Lapdev’s unique strength is that it manages the **entire environment lifecycle** — from creation to synchronisation — in a seamless, automated way. You don’t need to manually manage YAML files, sync resources, or worry about configuration drift.

### How It Works

Environments are created from **App Catalogs**, which serve as blueprints for your application.

When you create an environment:

1. Select an existing App Catalog that defines your app's workloads
2. Choose an environment type (Personal, Shared, or Branch)
3. Lapdev deploys the workloads:
   - Personal/Shared: into a dedicated namespace
   - Branch: into the shared base environment's namespace
4. ConfigMaps, Secrets, and Services are automatically included

The result: a fully functional copy of your app that mirrors production exactly, with zero manual YAML management.

> App Catalogs are created by reading production manifests directly from your cluster. Learn more about [**App Catalogs**](app-catalog.md).

### Environment Types

Lapdev supports three environment types, depending on your workflow and cost requirements:

* **Personal Environments** — fully isolated, ideal for independent development
* **Shared Environments** — team-wide baseline, ideal for integration testing
* **Branch Environments** — lightweight personal modifications on shared foundation, ideal for large teams

All types support **Devbox** for local development and debugging.

#### 1. Personal Environments

A **Personal Environment** is a completely isolated workspace for a single developer.

Each personal environment runs in its **own Kubernetes namespace**, so:

* One developer’s work never affects another’s.
* Multiple environments can be created per developer (e.g., one per feature or bug fix).
* You can switch between environments freely.

Because every personal environment contains a **full set of workloads**, it guarantees total isolation — but that comes with higher resource usage.

Lapdev helps mitigate this cost by allowing you to **pause and resume environments on demand**, automatically scaling down resources when you’re not using them.

**Best for:**\
Developers who need full isolation or want to test complex changes safely.

#### 2. Shared Environments

A **Shared Environment** is a team-wide baseline that runs a complete version of your app.

* Created and managed by admins
* Accessible by all team members
* Acts as the foundation for branch environments
* Ideal for integration testing or staging setups

**Best for:**\
Teams who need a stable, shared environment for testing or as a baseline for branch environments.

#### 3. Branch Environments

**Branch Environments** provide a cost-effective way to test changes by building on top of a shared environment.

When you create a branch environment:

* Initially, it references all workloads from the shared environment
* Only when you modify a service does Lapdev create a branched copy
* The branched copy runs alongside the shared version inside the shared environment's namespace, and Lapdev handles routing between them
* Unmodified services continue using the shared environment
* Multiple developers can work simultaneously without conflicts

This Git-like branching model means:

* **Near-instant creation** - no waiting for full environment deployment
* **Minimal cost** - only run what you're changing (up to 88% infrastructure reduction)
* **Realistic testing** - test against real production-like services, not mocks

**How it works:**

Lapdev uses intelligent traffic routing to direct requests to your branched services while everything else uses the shared baseline. This enables multiple developers to safely test changes in parallel.

**Requirements:**

* Services must communicate via HTTP/HTTPS
* Your application must forward `tracestate` headers in service calls
* Non-HTTP components (databases, message queues) remain shared

**Best for:**\
Large teams doing frequent feature development with cost efficiency in mind.

> For technical details on routing, sidecar proxies, and header propagation, see [**Branch Environment Architecture**](architecture/branch-environment-architecture.md).

### Local Development with Devbox

All environment types support **Devbox**, Lapdev's CLI tool for local development.

Devbox allows you to:

* **Intercept cluster traffic** to your local machine for real-time debugging
* Use your **local IDE** to edit, build, and debug as if running in the cluster
* **Access in-cluster resources** (databases, caches, internal APIs) transparently

When connected via Devbox, your local process integrates seamlessly with your environment. Other services automatically route traffic to your local instance, combining fast local iteration with accurate cluster integration.

> Learn more about [**Devbox**](devbox.md) and how to use it.

### Preview URLs

Every Lapdev environment can expose one or more **Preview URLs** — public HTTPS endpoints for accessing or sharing your running services.

Preview URLs are created **per service**. You choose which service to expose, and Lapdev automatically handles DNS, certificates, routing, and security.

When requests come in through a Preview URL in a branch environment, Lapdev automatically manages the **`tracestate` header** to route traffic to the correct branch environment.

> Learn more about [**Preview URLs**](preview-url.md).

### Comparison

| Feature         | Personal Environment                  | Shared Environment                   | Branch Environment              |
| --------------- | ------------------------------------- | ------------------------------------ | ------------------------------- |
| **Isolation**   | Fully isolated (per namespace)        | Shared by all team members           | Shared base, routed isolation   |
| **Cost**        | Highest                               | Medium                               | Lowest                          |
| **Ideal For**   | Deep testing, multi-service debugging | Integration testing, team staging    | Large teams, frequent branching |
| **Performance** | Slower to start, full workloads       | Full workloads, shared infrastructure| Instant setup, minimal overhead |
| **Access**      | Single developer                      | Entire team                          | Single developer (with shared base) |

### Summary

Lapdev Kubernetes Environments let you:

* Reproduce production exactly, without manual setup
* Stay notified when production changes - sync with one click when you're ready
* Choose between **full isolation**, **team-wide sharing**, or **cost-efficient branching**
* Use **Devbox** for fast, local-style development in Kubernetes
* Collaborate efficiently with consistent, shareable environments

Whether you're developing alone, testing as a team, or scaling across a large organization, Lapdev ensures every environment stays consistent, efficient, and production-accurate.
