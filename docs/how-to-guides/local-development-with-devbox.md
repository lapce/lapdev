# Local Development With Devbox

This guide shows how to set up and use **Devbox** for local development with your Lapdev environments.

#### Prerequisites

* Lapdev CLI installed.
* An active Lapdev environment (personal or branch).
* `kubectl` access to the connected cluster.

#### Install Devbox

```bash
curl -sSL https://get.lap.dev/install-devbox.sh | bash
```

#### Connect to an Environment

```bash
devbox connect <environment-name>
```

Devbox will:

* Authenticate and connect to the environment’s namespace.
* Set up secure routing between your local machine and the cluster.

_Placeholder screenshot:_

#### Intercept a Service

To reroute in-cluster traffic to your local process:

```bash
devbox intercept checkout-service
```

After interception:

* Requests to `checkout-service` in the cluster will route to your local code.
* You can make edits, hot-reload, or debug directly in your IDE.

_Placeholder screenshot:_

#### Access In-Cluster Services

While connected, you can
