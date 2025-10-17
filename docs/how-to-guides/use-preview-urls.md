# Use Preview URLs

Lapdev lets you create **Preview URLs** for your environments so you can securely access and share running services — without setting up DNS, ingress, or TLS certificates manually.

Each Preview URL points to a specific **service** inside your environment (for example, your `frontend`, `api-gateway`, or `admin` service).\
You can create multiple Preview URLs per environment, each targeting a different service.

### Prerequisites

Before you begin:

* You must have at least one **active environment** in Lapdev (personal, shared, or branch).
* The environment must show a status of **Active** in the Lapdev dashboard.
* Your app must expose at least one **Service** in Kubernetes.

### Create a Preview URL

1. Open the Lapdev dashboard: [https://app.lap.dev](https://app.lap.dev)
2. Go to the **Environments** tab.
3. Select the environment you want to create a preview for.
4. In the environment details page, scroll to the **Preview URLs** section.
5.  Click **Create Preview URL**.

    _Example screenshot:_\

6. In the **Create Preview URL** dialog:
   * Choose the **Service** you want this URL to point to (for example, `frontend`, `api-gateway`, or `admin`).
   * Optionally, enter a **Description** (e.g., “Frontend QA demo”).
   * Select the **Access Level** (private or public).
7. Click **Create**.

Lapdev will:

*   Assign a unique domain like

    ```
    mia-frontend-feature.app.lap.dev
    ```
* Automatically handle routing, DNS, and HTTPS for that URL
* Route traffic directly to the selected service inside your environment

### View and Open Preview URLs

Once created:

* All Preview URLs for the environment appear in the **Preview URLs** section.
* Each entry lists:
  * The **Service name** it targets
  * The **URL** itself
  * The **Access level** (private or public)

Click the URL to open it in your browser.

_Example screenshot:_

### Share the Preview URL

You can safely share a Preview URL with:

* Teammates or QA engineers for quick testing
* PMs or designers for feature reviews
* Automated test systems for integration runs

Just copy and share the link — no VPN, firewall, or cluster access needed.

> 💡 Tip: You can create multiple Preview URLs in the same environment if your app exposes multiple services (e.g., `frontend`, `admin`, `gateway`).

### Manage Access Control

Each Preview URL has its own access policy.

To update it:

1. In the **Preview URLs** section, click the settings icon next to a URL.
2. Choose who can access it:
   * **Private (recommended):** Only authenticated Lapdev users can view.
   * **Public:** Anyone with the link can view.
   * _(Coming soon)_ **Custom rules** for organization-level access.
3. Click **Save**.

> 🔒 Use private mode for internal branches or unreleased features.

### Delete a Preview URL

To remove a Preview URL:

1. Go to the **Preview URLs** section of your environment.
2. Click the **Delete** icon next to the URL.
3. Confirm deletion.

Deleting a Preview URL does **not** affect the environment or its workloads — it only removes that specific public endpoint.

### Troubleshooting

| Issue                    | Possible Cause                            | Solution                                                 |
| ------------------------ | ----------------------------------------- | -------------------------------------------------------- |
| Service not listed       | The service has no exposed port           | Check that the Kubernetes Service defines a valid `port` |
| Preview URL doesn’t open | Service not ready or endpoint unreachable | Verify pods and services are running                     |
| HTTPS warning            | Certificate still propagating             | Wait 30–60 seconds; Lapdev handles TLS automatically     |

### Next Steps

* Learn how Preview URLs work internally
* Use Devbox for real-time debugging connected to your environment
* Explore App Catalogs to define which workloads appear in your environments
