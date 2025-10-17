# Connect Your Kubernetes Cluster

This guide walks you through connecting your Kubernetes cluster to **Lapdev**, so Lapdev can manage development environments inside your cluster.

> **New to Clusters?** Read [**Cluster**](../core-concepts/cluster.md) to understand cluster roles, permissions, and how kube-manager works.

### Create a Cluster in the Lapdev Dashboard

1. Go to the Lapdev dashboard: [https://app.lap.dev](https://app.lap.dev)
2. Navigate to **Clusters** → click **Create New Cluster**.
3. Enter a **name** to identify your cluster (e.g. `staging-cluster` or `dev-cluster`).
4. After creating it, you’ll see an **authentication token** and **installation instructions** for your cluster.

Keep this token handy — it’s used to securely register your cluster with Lapdev.

### Install the Lapdev Kube Manager

Run the following commands in your terminal:

```bash
kubectl create namespace lapdev
kubectl apply -f https://get.lap.dev/lapdev-kube-manager.yaml
```

This deploys the `lapdev-kube-manager` controller that securely connects your cluster to Lapdev.

### Configure Cluster Permissions

After the cluster is connected, configure which environment types can be deployed to this cluster:

1. Go to the cluster details page in the dashboard
2. Find the **Permissions** section
3. Toggle permissions based on your needs:
   * **Personal Environments** - Allow developers to create isolated environments
   * **Shared Environments** - Allow team-wide baseline and branch environments

You can change these settings at any time.

> Learn more about cluster permissions and use cases in [**Cluster**](../core-concepts/cluster.md).

### Verify Connection

Once the installation is complete:

* Go back to the **Clusters** page in the Lapdev dashboard.
* Your cluster should appear in the list with a status of **Active**.

If the status doesn’t update after a minute, double-check that:

*   The `lapdev-kube-manager` pod is running:

    ```bash
    kubectl get pods -n lapdev
    ```
* Your network allows outbound HTTPS connections to `api.lap.dev`.

### Next Steps

Your cluster is now connected to Lapdev! 🎉

You can start:

* Creating an [App Catalog](create-an-app-catalog.md) from your cluster's workloads
* Creating [Environments](create-lapdev-environment.md) from your App Catalog
* Learn more about [Cluster concepts](../core-concepts/cluster.md) and architecture
