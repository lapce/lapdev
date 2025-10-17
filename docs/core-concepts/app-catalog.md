# App Catalog

The **App Catalog** is the foundation of how Lapdev understands and manages your application in Kubernetes.

It defines **which workloads belong to your app** — such as deployments, statefulsets, and their associated configuration — and acts as the **blueprint** Lapdev uses to create consistent and production-aligned development environments.

### Why App Catalogs

In most Kubernetes setups, application configuration is scattered across multiple manifests or Helm charts.\
Developers often need to manually manage which workloads belong to which app, and this can easily lead to configuration drift between environments.

The App Catalog solves this by:

* **Defining your app once** — directly from your production workloads.
* **Reusing that definition** to create any number of environments (personal, shared, or branch).
* **Keeping everything in sync** with your production manifests — no duplicate YAML or manual updates.

### How It Works

1. Lapdev connects to your **source cluster** and reads all workloads running in it.
2. You select the workloads that make up your application (for example, `frontend`, `api`, and `database`).
3. Lapdev groups those workloads — along with their related **ConfigMaps**, **Secrets**, and **Services** — into an **App Catalog**.
4. The catalog is stored in Lapdev and can be reused to create environments on any connected cluster.

> 💡 The **source cluster** is typically your staging or production cluster.\
> You can also use a dedicated cluster that mirrors production manifests.

### Relationship Between App Catalogs and Environments

| Concept         | Purpose                                                                       | Example                                                                  |
| --------------- | ----------------------------------------------------------------------------- | ------------------------------------------------------------------------ |
| **App Catalog** | Defines what your app is — a collection of workloads and their configuration. | `checkout-service` (includes `checkout-api`, `payment-api`, `frontend`)  |
| **Environment** | A running instance of that app in a specific cluster.                         | `mia-checkout-feature` (personal env) or `staging-checkout` (shared env) |

You can think of an **App Catalog** as a _template_ and an **Environment** as a _live deployment_ of that template.

### Editing an App Catalog

After creating an App Catalog, you can modify it anytime:

* Add or remove workloads
* Update its description
* Sync it with your source cluster to reflect production changes

Lapdev automatically tracks these updates and keeps associated environments consistent with the latest configuration.

### Typical Usage Pattern

App Catalogs fit into your workflow like this:

```
Source Cluster (Production/Staging)
        ↓
   App Catalog (Blueprint)
        ↓
Multiple Environments (Personal/Shared/Branch)
```

Once created, catalogs can be synced when production manifests change, keeping all environments up to date.

> For step-by-step instructions, see [**Create an App Catalog**](../how-to-guides/create-an-app-catalog.md).

### Benefits of App Catalogs

✅ **No config drift:** Use production manifests as the single source of truth.\
✅ **Reusable blueprint:** Create multiple environments from the same catalog.\
✅ **Simplified management:** Centralize workloads and configuration in one place.\
✅ **Scalable:** Works across clusters and environment models.

### When to Use Multiple App Catalogs

You can create more than one App Catalog if:

* Your organization manages multiple independent apps (e.g. `checkout`, `search`, `analytics`).
* You want to separate internal tools from core services.
* Different teams own different parts of your system.

Each App Catalog can have its own lifecycle and permissions.
