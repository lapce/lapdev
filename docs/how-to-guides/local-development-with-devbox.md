# Local Development With Devbox

This guide shows how to set up and use **Devbox** for local development with your Lapdev environments.

> **New to Devbox?** Read [**Devbox**](../core-concepts/devbox.md) to understand what it does and when to use it.

### Prerequisites

Before using Devbox, you need:

* **Lapdev CLI installed** - Devbox is built into the `lapdev` command
* **An active Lapdev environment** - Personal, shared, or branch environment

### Install the Lapdev CLI

Run one of the following commands based on your operating system.

#### Linux or macOS

```bash
curl -fsSL https://get.lap.dev/lapdev-cli.sh | bash
```

This script detects your architecture, downloads the latest release, and places the `lapdev` binary in `/usr/local/bin` (or `~/.local/bin` if needed). Re-run it at any time to upgrade or pass `--version <x.y.z>` to pin a specific build.

#### Windows PowerShell

```powershell
irm https://get.lap.dev/lapdev-cli.ps1 | iex
```

Run the command in an elevated PowerShell window if your execution policy requires it. The installer copies `lapdev.exe` into `%LOCALAPPDATA%\Lapdev\bin` and appends that directory to your user `PATH`. Restart your shell (or VS Code terminal) after installation so the new path is picked up.

After installing, verify everything is ready with:

```bash
lapdev --version
```

### Connect to Lapdev

First, connect your Devbox CLI to Lapdev:

```bash
lapdev devbox connect
```

This establishes a secure connection between your local machine and Lapdev.

### Set Active Environment

After connecting, set which environment you want to work with in the Lapdev dashboard:

1. Go to the **Environments** tab
2. Select the environment you want to use
3. Click **Set as Active Environment**

Once set, all traffic interception and service access will route through this active environment.

_Placeholder screenshot:_

### Intercept a Service

Once connected, you can intercept traffic for a specific workload through the Lapdev dashboard:

1. Open your environment in the Lapdev dashboard
2. Stay on (or switch to) the **Workloads** tab
3. Locate the workload that owns the service you want to intercept (e.g., `checkout-service`)
4. Click **Start Intercept** inside that workload’s row

Lapdev automatically mirrors each container port, reusing any overrides from your most recent intercept. If you need custom local ports after starting the intercept, use the **Edit Ports** button on the intercept card and update the mappings there.

After enabling interception:

* All cluster traffic for that service routes to your local machine
* You can make edits, hot-reload, or debug directly in your IDE
* Other services in the cluster remain unaffected

_Placeholder screenshot:_

### Access In-Cluster Services

While connected via Devbox, you can access any service in your environment using their cluster DNS names (e.g., `postgres-service:5432`, `redis-service:6379`) directly from your local code. Devbox handles the tunneling automatically — no port forwarding or VPN needed.

### Next Steps

* Learn more about [Devbox architecture](../core-concepts/devbox.md)
* Understand [traffic routing](../core-concepts/architecture/traffic-routing-architecture.md)
* Create [preview URLs](use-preview-urls.md) to share your work
