# Create Lapdev Environment

A **Lapdev Environment** is a running instance of your app inside a Kubernetes cluster.

It's created from an existing **App Catalog**, which defines which workloads make up your application.

This guide shows how to create **personal** and **shared** environments from an App Catalog, and how to create **branch** environments from a shared environment.

> **New to Environments?** Read [**Environment**](../core-concepts/environment.md) to understand environment types and how they work.

### Prerequisites

Before creating an environment:

* You must have at least one **connected Kubernetes cluster** (Active in the Lapdev dashboard).
*   You must have an existing **App Catalog** that defines your app's workloads.

    > If you haven't created one yet, see [**Create an App Catalog**](create-an-app-catalog.md).

### Start from an App Catalog (Personal or Shared)

1. Go to the **App Catalogs** tab in the Lapdev dashboard.
2. Select the catalog you want to use as the base for your environment.
3. Click **Create Environment**.

_Example screenshot:_

### Select Environment Type (Personal or Shared)

Lapdev supports three environment types depending on your workflow and cost needs. When you create an environment from an App Catalog you can choose between:

#### **Personal Environment**

* A fully isolated copy of your app, deployed into a per-developer namespace.
* Ideal for local testing, debugging, or experimentation without affecting others.

#### **Shared Environment**

* A single shared version of your app that multiple developers can access.
* Useful for integration testing or a team staging setup.

Branch environments are created from an existing shared environment. See the next section for that workflow.

### Create a Branch Environment

Branch environments reuse an existing shared environment as their baseline.

1. Go to the **Environments** tab in the Lapdev dashboard.
2. Open the shared environment you want to branch from.
3. Click **Create Branch Environment**.
4. Select the services you plan to modify and provide a name/description for the branch.
5. Click **Create**.

Only the services you modify are duplicated into your branch namespace; everything else continues to run in the shared environment.

> 🧠 Use **branch environments** for feature work — they're lightweight, fast to spin up, and cost-efficient.

### Verify and Access Your Environment

After creation:

* The environment will appear in the **Environments** list.
* Lapdev automatically provisions:
  * All workloads from the App Catalog
  * Associated ConfigMaps, Secrets, and Services
  * Kubernetes namespace and networking

You can monitor status, logs, and sync state directly from the dashboard.

### Next Steps

Your environment is ready! You can now:

* Create [Preview URLs](use-preview-urls.md) for HTTPS access to your services
* Use [Devbox](local-development-with-devbox.md) to connect locally for live debugging
* Learn more about [Environments](../core-concepts/environment.md)
