# Preview URL

A **Preview URL** is a unique, automatically generated HTTPS endpoint that lets you access your Lapdev environment directly from the web — without any manual DNS or Ingress configuration.

Every Lapdev environment, whether **personal**, **shared**, or **branch**, gets its own Preview URL.\
This makes it easy to preview changes, share work with teammates, and test production-like behavior in real time.

### Why Preview URLs

In traditional Kubernetes setups, exposing your app for testing often means:

* Creating or editing Ingress rules
* Configuring DNS records
* Managing TLS certificates
* Waiting for ops to approve changes

That’s slow, error-prone, and not scalable for dozens of developers or short-lived environments.

Lapdev solves this with **automatic Preview URLs** — secure, per-environment endpoints that “just work.”

### How It Works

When you create an environment, Lapdev:

1. Detects your app’s exposed **Services** (e.g. `frontend`, `api`, `gateway`).
2.  Automatically provisions a **unique domain** for that environment, such as:

    ```
    alice-checkout-feature.app.lap.dev
    ```
3. Configures HTTPS certificates automatically — no `cert-manager`, DNS setup, or manual YAML needed.
4. Routes traffic through Lapdev’s managed proxy layer directly to your workloads inside the target cluster.

All routing and TLS termination are handled by Lapdev’s control plane, so your cluster stays secure and simple.

### Domain and Structure

Preview URLs follow a consistent, human-readable pattern:

```
<user-or-branch>-<app-name>.app.lap.dev
```

Examples:

* `mia-checkout-feature.app.lap.dev` → personal or branch environment
* `staging-checkout.app.lap.dev` → shared environment

This makes it easy to tell what you’re looking at — and safe to share with your team.

### Access Control

By default, Preview URLs are public, but Lapdev allows optional access control:

* **Authenticated access only:** Only Lapdev users in your organization can open the URL.
* **Public preview:** Anyone with the link can view it.
* **Custom rules (coming soon):** Integrate with your identity provider for fine-grained access policies.

Access settings are managed per environment in the Lapdev dashboard.

### Benefits

✅ **Instant access:** No waiting for ops or configuring ingress.\
✅ **Secure by default:** Auto-managed HTTPS and certificates.\
✅ **Shareable:** Send links to PMs, QA, or teammates easily.\
✅ **Isolated:** Each environment’s URL maps only to its own workloads.\
✅ **Consistent:** Same domain and routing system across all environments.

### How It Relates to Other Lapdev Components

| Component       | Role                                                                        |
| --------------- | --------------------------------------------------------------------------- |
| **App Catalog** | Defines which workloads are part of the app.                                |
| **Environment** | A running instance of the app in Kubernetes.                                |
| **Preview URL** | Provides web access to that environment — with automatic routing and HTTPS. |

### Example Flow

1. You create a **personal environment** from your App Catalog.
2. Lapdev deploys the workloads and assigns a Preview URL automatically.
3. You share that URL with a teammate — no ingress, no firewall changes, just click and open.
