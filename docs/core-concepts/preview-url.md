# Preview URL

A **Preview URL** is a unique, automatically generated HTTPS endpoint that lets you access services in your Lapdev environment directly from the web — without any manual DNS or Ingress configuration.

You can create Preview URLs for any environment type (**personal**, **shared**, or **branch**), exposing specific services for access and sharing.\
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

When you create a Preview URL for a service in your environment, Lapdev:

1. Detects the service you want to expose (e.g. `frontend`, `api`, `gateway`)
2. Automatically generates a unique HTTPS domain for that service
3. Configures TLS certificates automatically — no `cert-manager`, DNS setup, or manual YAML needed
4. Routes traffic through Lapdev's managed proxy layer directly to your service inside the cluster

All routing and TLS termination are handled by Lapdev's control plane, so your cluster stays secure and simple.

Each Preview URL is unique and automatically managed by Lapdev, making it safe to share with your team.

### Access Control

Preview URLs default to **Organization** access, but you can configure per Preview URL:

* **Organization (default):** Only members of your Lapdev organization can access (after login)
* **Public:** Anyone with the link can access without authentication

Access settings are managed per Preview URL in the Lapdev dashboard.

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

### Usage Pattern

Preview URLs make collaboration effortless:

* Create an environment and expose the services you want to share
* Lapdev generates secure HTTPS URLs automatically
* Share URLs with teammates, QA, or stakeholders instantly
* No infrastructure changes, ingress configuration, or firewall rules needed

> For step-by-step instructions, see [**Use Preview URLs**](../how-to-guides/use-preview-urls.md).
