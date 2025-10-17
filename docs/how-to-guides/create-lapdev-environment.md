# Create Lapdev Environment

A **Lapdev Environment** is a running instance of your app inside a Kubernetes cluster.

It’s created from an existing **App Catalog**, which defines which workloads make up your application.

This guide walks you through how to create personal, shared, and branch environments from an App Catalog.

### Prerequisites

Before creating an environment:

* You must have at least one **connected Kubernetes cluster** (Active in the Lapdev dashboard).
*   You must have an existing **App Catalog** that defines your app’s workloads.

    > If you haven’t created one yet, follow Create an App Catalog.

### Start from an App Catalog

1. Go to the **App Catalogs** tab in the Lapdev dashboard.
2. Select the catalog you want to use as the base for your environment.
3. Click **Create Environment**.

_Example screenshot:_

### Select Environment Type

Lapdev supports three environment types depending on your workflow and cost needs.

#### **Personal Environment**

* A fully isolated copy of your app, deployed into a per-developer namespace.
* Ideal for local testing, debugging, or experimentation without affecting others.

#### **Shared Environment**

* A single shared version of your app that multiple developers can access.
* Useful for integration testing or a team staging setup.

#### **Branch Environment**

* Built from an existing **Shared Environment**.
* Includes only workloads you've changed; unmodified services reuse the shared environment.
* Created directly from the **Shared Environment details page** — click **Create Branch Environment**.

> **Note:** You must first create a Shared Environment before you can create Branch Environments from it.

> 🧠 Use **branch environments** for feature work — they're lightweight, fast to spin up, and cost-efficient.

### Verify and Access Your Environment

After creation:

* The environment will appear in the **Environments** list.
* Lapdev automatically provisions:
  * All workloads from the App Catalog
  * Associated ConfigMaps, Secrets, and Services
  * Unique HTTPS preview URLs for accessing your services

You can monitor status, logs, and sync state directly from the dashboard.

### Next Steps

Your environment is ready! You can now:

* Use Devbox CLI to connect locally for live debugging.
* Sync environment configuration with production updates.
* Manage or delete environments when you’re done.
