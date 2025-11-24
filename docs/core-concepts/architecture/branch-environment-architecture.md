# Branch Environment Architecture

Branch environments are a cost-effective way to run development environments in Kubernetes. Instead of duplicating all services for each developer, branch environments build on a [shared environment](../environment.md) and only run the services you're actively modifying.

### Architecture Diagram

<img src="../../.gitbook/assets/file.excalidraw (3).svg" alt="" class="gitbook-drawing" />

### Concept

Without Branch Environments:

```
10 developers × 100 services each = 1000 pods total
Every developer runs a complete copy of all services
```

With Branch Environments:

```
Shared environment: 100 services (shared by everyone)
+ 10 developers × 2 modified services each = 20 branched services
Total: 120 pods
```

**Example:** Alice modifies `api`, Bob modifies `worker` - they only run their modified services while sharing the rest.

**Savings: 88% reduction in infrastructure costs**

### Prerequisites

Branch environments require:

* **HTTP/HTTPS communication only** - Non-HTTP components (databases, message queues, gRPC) remain shared across all branches
* **Header propagation** - Your application must forward OpenTelemetry `tracestate` headers in service-to-service calls

**Important:** While Lapdev automatically injects the tracestate header for incoming requests, your application code must forward headers in service-to-service HTTP calls for routing to work correctly.

### How It Works

#### The Shared Environment

One shared environment runs all your services:

* Contains a complete copy of all workloads
* Shared by all developers
* Acts as the foundation for all branch environments

#### Your Branch Environment

When you create a branch, you specify which service(s) you're modifying:

* Only those services run as branched workloads alongside the shared copies in the same namespace
* Everything else routes to the shared environment

#### Traffic Routing Overview

When someone accesses your branch [preview URL](../preview-url.md), Lapdev routes traffic intelligently:

* Requests to services you've modified → route to your branch version
* Requests to all other services → route to the shared environment

**Example:** If Alice's branch modifies only the `api` service, then requests for `api` go to her branch while `web`, `worker`, and all other services use the shared environment.

#### Routing Mechanism

**1. Header Injection**

When a request enters through a [preview URL](../preview-url.md), Lapdev automatically injects an OpenTelemetry `tracestate` header that identifies which branch the request belongs to.

**2. Header Propagation (Your Responsibility)**

Your application must forward the `tracestate` header in all HTTP service-to-service calls.

**How to implement:**

Most HTTP clients automatically forward headers when you pass them explicitly. For example:

```javascript
// Forward all incoming headers to downstream services
const response = await axios.get('http://other-service:8080/api', {
  headers: request.headers  // This includes the tracestate header
});
```

**Framework-specific approaches:**

✅ **Automatic with these frameworks:**
- Spring Cloud Sleuth (auto-propagates OpenTelemetry headers)
- OpenTelemetry-instrumented applications (SDK handles propagation)

⚠️ **Manual forwarding required:**
- Express.js, Fastify, Koa (pass `request.headers` to HTTP client)
- Go HTTP clients (copy headers from incoming request)
- Any custom HTTP client implementation

**3. Routing Decision**

The Lapdev Sidecar Proxy (automatically injected by Lapdev into each pod in managed environments) sits in front of your workload traffic:

1. Intercepts incoming requests headed to your service
2. Reads the `tracestate` header to identify the branch
3. Checks if the target service has a branch override for this branch
4. If a Devbox intercept is active for this branch/service, routes to the developer’s local process; otherwise routes to the branched service if present
5. Falls back to the shared environment if no branch-specific target exists
6. The header continues downstream, so the next hop can repeat the decision

> **Note:** The sidecar proxy is automatically added when Lapdev creates your environment. No manual configuration needed. For more details, see [Traffic Routing Architecture](traffic-routing-architecture.md).

### Key Benefits

* **Cost-effective:** Only run what you're changing (up to 88% infrastructure reduction)
* **Instant creation:** Branch environments are created instantly - branched workloads are only deployed when you modify them
* **Realistic testing:** Test your changes against real production-like services, not mocks
* **No conflicts:** Multiple developers can work on different services simultaneously

### Limitations

#### 1. Header Propagation Required

Your application must forward the `tracestate` header in service-to-service HTTP calls, otherwise routing will break mid-request chain. See the **Routing Mechanism** section above for implementation guidance.

#### 2. HTTP/HTTPS Traffic Only

Branch routing works exclusively for HTTP/HTTPS communication. Other protocols are always shared:

* **Shared across all branches:** Databases, message queues, gRPC services, TCP connections
* **Implication:** Be careful with database schema changes or queue message format changes

#### 3. Stateful Services Considerations

Services with persistent state (databases, caches) are shared across branches:

* Database writes from one branch affect all other branches
* Redis/Memcached entries are visible to all branches
* File uploads or background jobs may interfere between branches

**Best Practice:** Use branch-specific prefixes for cache keys and database records when testing data modifications.

### Troubleshooting

#### Traffic not routing correctly to my branch

**Symptoms:**
- Changes aren't visible when accessing your branch preview URL
- Some requests use your branch version, others use the shared version (inconsistent routing)

**Possible causes:**

1. **Headers not propagated:** Your services aren't forwarding the `tracestate` header in HTTP calls
2. **Service not branched:** The service is not actually deployed in your branch environment
3. **Wrong preview URL:** You're accessing the shared environment URL instead of your branch URL

**Debug steps:**

* Verify your HTTP clients forward incoming headers to downstream services (see example above)
* Check the Lapdev dashboard to confirm which services are branched in your environment
* Check sidecar proxy logs to see routing decisions
* Look for the `tracestate` header in your service logs - if it's missing after the first hop, headers aren't being propagated

#### Database changes from my branch affect other developers

This is **expected behavior**. Branch environments share databases and other stateful services.

**Solutions:**

* Use branch-specific database prefixes or namespaces
* Test with read-only database operations
* Use separate test databases per branch (requires manual configuration)

### Learn More

**Related Architecture Documentation:**
* [Traffic Routing Architecture](traffic-routing-architecture.md) - Detailed explanation of how routing works across all environment types
* [Architecture Overview](README.md) - Overall system design and component interactions

**Core Concepts:**
* [Environment](../environment.md) - Understanding Personal, Shared, and Branch environments
* [Preview URL](../preview-url.md) - HTTPS access to your services
* [App Catalog](../app-catalog.md) - Blueprint for your application

**How-To Guides:**
* [Create Lapdev Environment](../../how-to-guides/create-lapdev-environment.md) - How to create branch environments
* [Use Preview URLs](../../how-to-guides/use-preview-urls.md) - Access and share your branch environment
