# Connect Your Kubernetes Cluster

This guide walks you through connecting your Kubernetes cluster to **Lapdev**, so Lapdev can manage development environments inside your cluster.

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

> 💡 The `lapdev-kube-manager` is a lightweight controller that securely connects your Kubernetes cluster to Lapdev.\
> It runs inside the `lapdev` namespace and handles synchronisation, environment creation, and traffic routing. You can read more about how it works in our [Architecture doc](../core-concepts/architecture/).

### Configure Cluster Permissions

After the cluster is created, you’ll see **permission settings** in the cluster details page.

These control [**what kinds of environments**](../core-concepts/environment.md) can be deployed to this cluster:

* **Personal Environments:**\
  Allow individual developers to create isolated environments directly in this cluster. Each developer’s workloads, ConfigMaps, and Secrets will be namespaced separately for full isolation.
* **Shared Environments:**\
  Allow shared or branch environments that multiple developers can use together — ideal for cost-efficient setups.

You can toggle these permissions **on or off** at any time, depending on how you want the cluster to be used.

For example:

* Enable both for a staging or dev cluster.
* Enable only shared environments for a pre-prod cluster used by the whole team.
* Enable only personal environments for testing or private development.

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

* Creating [environments](../core-concepts/environment.md)
* Using [Devbox CLI](local-development-with-devbox.md) for local development (link to CLI doc)
