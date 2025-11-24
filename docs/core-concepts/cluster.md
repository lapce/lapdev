# Cluster

A **Cluster** in Lapdev refers to a Kubernetes cluster you've connected to Lapdev for managing development environments.

Clusters can serve two roles:
- **Source Cluster** - Provides production manifests for creating App Catalogs
- **Target Cluster** - Hosts development environments

The same cluster can serve both roles, which is the most common setup.

### Why Connect Clusters

Connecting your Kubernetes cluster to Lapdev enables:

* **App Catalog Creation** - Read production manifests to define your app
* **Environment Deployment** - Run development environments in the cluster
* **Production Parity** - Ensure dev environments match production exactly
* **Centralized Management** - Manage multiple clusters from one dashboard

### How It Works

#### Lapdev Kube Manager

When you connect a cluster, Lapdev deploys a lightweight controller called **lapdev-kube-manager** inside your cluster.

The kube-manager:

* **Establishes secure connection** - Opens a WebSocket tunnel to Lapdev API Server (TLS encrypted)
* **Reads production manifests** - Discovers Deployments, StatefulSets, ConfigMaps, Secrets, and Services
* **Creates dev environments** - Replicates selected workloads into isolated or shared namespaces
* **Handles traffic routing** - For branch environments, routes traffic to the correct version of services
* **Manages synchronization** - Keeps environments updated with production changes

**Security Model:**

* Connection is **outbound-only** from your cluster (no inbound firewall rules needed)
* Lapdev API Server **never** accesses your cluster's API server directly
* Kube-manager has **read access** to production namespaces
* Kube-manager has **full access** to Lapdev-managed namespaces only
* Cannot access other cluster resources or namespaces

#### Token-Based Authentication

Each cluster is authenticated using a unique token:

* Generated when you create the cluster in Lapdev dashboard
* Used by kube-manager to establish the secure tunnel
* Stored as a Kubernetes Secret in your cluster

### Cluster Roles

#### Source Cluster

A **source cluster** is where Lapdev reads production manifests to create App Catalogs.

* Typically your **staging** or **production** cluster
* Contains the workloads you want to replicate in dev environments
* Kube-manager reads manifests but doesn't modify them
* You can designate any cluster as a source

**Common setups:**
* Use production cluster as source (safest, always up-to-date)
* Use staging cluster as source (if staging mirrors production)
* Use dedicated "template" cluster (for controlled rollout)

#### Target Cluster

A **target cluster** is where Lapdev deploys development environments.

* Can be the same as your source cluster or different
* Hosts all Personal, Shared, and Branch environments
* Kube-manager creates and manages namespaces here
* Choose based on resource availability and cost

**Common setups:**
* Same cluster as source (simplest, most common)
* Dedicated dev cluster (isolates dev from production)
* Multiple dev clusters (for different teams or regions)

### Cluster Permissions

Cluster permissions control **which types of environments** can be deployed to a cluster.

#### Personal Environments Permission

When enabled, developers can create **Personal Environments** in this cluster.

* Each developer gets fully isolated namespaces
* Higher resource usage (every dev runs complete app)
* Best for: staging/dev clusters with sufficient resources

**Enable when:**
* Developers need full isolation for testing
* Cluster has resources for multiple complete environments
* You want individual developers to experiment freely

#### Shared Environments Permission

When enabled, admins can create **Shared Environments** and developers can create **Branch Environments** in this cluster.

* Shared environments are team-wide baselines
* Branch environments are lightweight personal modifications
* Lower resource usage (shared baseline + modifications only)
* Best for: cost-efficient development at scale

**Enable when:**
* Team uses branch environment workflow
* You want cost-efficient development environments
* Multiple developers work on the same application

#### Permission Use Cases

| Cluster Type | Personal | Shared | Use Case |
|-------------|----------|---------|-----------|
| Dev Cluster | ✅ | ✅ | Full flexibility for developers |
| Staging Cluster | ❌ | ✅ | Team-wide testing baseline only |
| Testing Cluster | ✅ | ❌ | Isolated testing environments |
| Production | ❌ | ❌ | Source only, no dev environments |

> You can change these permissions at any time in the cluster settings.

### Relationship to Other Components

Clusters are foundational to Lapdev's workflow:

```
Source Cluster
    ↓ (provides manifests)
App Catalog
    ↓ (blueprint)
Target Cluster
    ↓ (deploys to)
Environments (Personal/Shared/Branch)
```

| Component | Role with Clusters |
|-----------|-------------------|
| **Cluster** | Provides infrastructure and manifests |
| **App Catalog** | Created by reading manifests from source cluster |
| **Environment** | Deployed into target cluster using App Catalog |
| **Kube Manager** | Runs inside cluster, bridges to Lapdev API |

### Multiple Clusters

You can connect multiple clusters to Lapdev for different purposes:

**Common multi-cluster setups:**

* **Source + Multiple Targets:**
  - Production cluster (source only)
  - Dev cluster A (target for team A)
  - Dev cluster B (target for team B)

* **Regional Separation:**
  - US cluster (source + target)
  - EU cluster (target only, for EU developers)

* **Environment Segregation:**
  - Staging cluster (source + shared environments)
  - Dev cluster (personal environments only)

Each cluster appears in your Lapdev dashboard with its own status, permissions, and environments.

### Next Steps

Ready to connect your cluster? See [**Connect Your Kubernetes Cluster**](../how-to-guides/connect-your-kubernetes-cluster.md) for step-by-step instructions.
