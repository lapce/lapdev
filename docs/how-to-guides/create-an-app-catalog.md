# Create an App Catalog

An **App Catalog** defines which workloads make up your application — it's the blueprint Lapdev uses to create development environments.

This guide walks you through creating an App Catalog from your connected cluster.

> To understand why App Catalogs exist and how they fit into Lapdev, read [**App Catalog**](../core-concepts/app-catalog.md) first.

### Prerequisites

Before creating an App Catalog, make sure:

* You've connected at least one **Kubernetes cluster** to Lapdev.
* The cluster you'll read workloads from shows as **Active** in the Lapdev dashboard.
* The cluster contains the workloads you want to include (e.g. your production or staging workloads).

> Don't have a cluster connected yet? See [**Connect Your Kubernetes Cluster**](connect-your-kubernetes-cluster.md).

> 💡 The same cluster can be used later for both reading workloads **and** deploying environments.

### Open the Cluster in the Dashboard

1. Go to [https://app.lap.dev](https://app.lap.dev).
2. Navigate to the **Clusters** tab.
3. Click the cluster you want to use as the **source**.

### Select Workloads

Once inside the cluster details page:

1. Lapdev lists all **workloads** (Deployments, StatefulSets, DaemonSets, etc.) discovered in the cluster.
2. You can:
   * Filter workloads by **Namespace** or **Workload Type**.
   * Optionally check **Show System Workloads** to include system components.
3. Select the workloads that make up your app.

<figure><img src="../.gitbook/assets/Screenshot 2025-11-19 at 20-27-19 Lapdev Dashboard.png" alt=""><figcaption></figcaption></figure>



### Create the App Catalog

1. After selecting workloads, click **Create App Catalog**.
2. In the dialog:
   * Enter a **Name** for the catalog (e.g. `checkout-service`, `internal-tools`).
   * Optionally add a **Description**.
   * Review the list of selected workloads.
3. Click **Create**.

<figure><img src="../.gitbook/assets/Screenshot 2025-11-19 at 20-30-01 Lapdev Dashboard.png" alt=""><figcaption></figcaption></figure>

Lapdev will create the catalog and register it in the **App Catalogs** section of your dashboard.

> 💡 Don’t worry if you missed something — you can edit the catalog later to add or remove workloads.

### Verify Your App Catalog

1. Go to the **App Catalogs** tab in the dashboard.
2. You’ll see your newly created catalog listed with:
   * The **name** and **description**
   * The **number of workloads** included
   * The **source cluster**

Click a catalog name to view its details.

_Example screenshot:_

### Next Steps

Once your App Catalog is ready, you can:

* Create a [Lapdev Environment](create-lapdev-environment.md) from it
* Learn about [Environment types](../core-concepts/environment.md)
* Understand [Cluster roles](../core-concepts/cluster.md) for multi-cluster setups
