# Architecture: Preview URLs and Branch Environments

## Table of Contents
1. [Overview](#overview)
2. [Preview URLs Architecture](#preview-urls-architecture)
3. [Branch Environments Architecture](#branch-environments-architecture)
4. [Cost Optimization Through Resource Sharing](#cost-optimization-through-resource-sharing)
5. [Database Schema](#database-schema)
6. [Request Flow Diagrams](#request-flow-diagrams)
7. [Security and Access Control](#security-and-access-control)
8. [API Reference](#api-reference)
9. [Key Files Reference](#key-files-reference)
10. [Design Decisions](#design-decisions)

---

## Overview

Lapdev's Kubernetes integration supports two key features for development workflows:

1. **Preview URLs**: Publicly accessible URLs that expose specific services from Kubernetes environments
2. **Branch Environments**: Cost-efficient isolated development environments that share resources with base environments

These features enable a GitOps-style workflow where developers can:
- Create isolated environments for feature branches **without duplicating entire infrastructure**
- Customize only the workloads they need to modify
- Expose services via unique URLs with granular access control
- **Save significant cloud costs** by sharing unmodified services with the base environment

### Key Cost-Saving Principle

**Branch environments only deploy what changes** - unmodified workloads are shared with the base environment. This is fundamentally different from creating isolated namespaces where every service must be duplicated.

**Example:**
```
Base Environment "staging" (10 services):
  - frontend (1 GB RAM)
  - api (2 GB RAM)
  - auth-service (512 MB RAM)
  - payment-service (1 GB RAM)
  - notification-service (512 MB RAM)
  - analytics-service (2 GB RAM)
  - search-service (4 GB RAM)
  - cache-service (2 GB RAM)
  - queue-worker (1 GB RAM)
  - cron-jobs (512 MB RAM)

  Total: ~15 GB RAM

Branch Environment "feature-login":
  - Modifies: frontend + auth-service
  - Deploys: Only frontend (1 GB) + auth-service (512 MB)
  - Shares: All other 8 services with base

  Total: ~1.5 GB RAM (90% cost reduction!)
```

---

## Preview URLs Architecture

### URL Format

Preview URLs follow this pattern:
```
https://{service-name}-{port}-{random-hash}.app.lap.dev
```

**Example:**
```
https://webapp-8080-abc123def456.app.lap.dev
```

**Components:**
- `service-name`: Kubernetes service name (e.g., "webapp")
- `port`: Service port number (e.g., "8080")
- `random-hash`: 12-character random identifier for uniqueness

### URL Generation

When creating a preview URL:

1. User selects a service and port from their environment
2. System generates subdomain: `{service.name}-{port}-{generate_random_12_chars()}`
3. Subdomain is globally unique (enforced by database constraint)
4. Full URL is returned to user

**Implementation:** `/workspaces/lapdev/crates/kube/src/preview_url.rs`

### URL Resolution Flow

```mermaid
graph TD
    A[Incoming Request] --> B[Parse Host Header]
    B --> C[Extract Subdomain Components]
    C --> D{Parse Format}
    D -->|Success| E[Query Environment by Hash]
    D -->|Fail| F[Return 404]
    E --> G[Find Service by Name]
    G --> H[Find Preview URL Config]
    H --> I{Validate Access}
    I -->|Authorized| J[Return Target Info]
    I -->|Unauthorized| K[Return 403]
```

**Resolution Steps:**

1. **Parse Subdomain**
   - Split on `-` delimiter
   - Extract: service name (all but last 2 parts), port (2nd to last), hash (last)

2. **Find Environment**
   - Query `kube_environment` table using environment hash
   - Currently uses `name` field (TODO: add dedicated hash field)

3. **Find Service**
   - Query `kube_environment_service` by `environment_id` and `service_name`

4. **Find Preview URL Configuration**
   - Query `kube_environment_preview_url` by `environment_id`, `service_id`, and `port`

5. **Validate Access Level**
   - Check user permissions based on access level (Personal/Shared/Public)

6. **Return Target**
   - Returns `PreviewUrlTarget` with cluster, namespace, service, port info

### HTTP Proxy Implementation

**File:** `/workspaces/lapdev/crates/kube/src/http_proxy.rs`

The `PreviewUrlProxy` is a standalone TCP-based proxy server:

**Architecture:**
- Listens on configured TCP port (e.g., 8080)
- Accepts incoming connections
- Parses HTTP request headers to extract `Host`
- Resolves preview URL to target service
- Opens tunnel to target via `TunnelRegistry`
- Performs bidirectional TCP streaming

**Key Features:**

1. **TCP-Level Proxying**
   - Does NOT parse full HTTP requests/responses
   - Forwards raw TCP streams bidirectionally
   - Supports WebSocket upgrades and streaming protocols

2. **Request Tracking**
   - Adds `tracestate` header: `lapdev-env-id={environment_id}`
   - Enables distributed tracing through proxy chain

3. **Connection Management**
   - Each connection gets unique tunnel ID
   - Automatic cleanup on connection close
   - Updates `last_accessed_at` timestamp for analytics

4. **Error Handling**
   - Returns HTTP error responses for resolution failures
   - Logs errors with tunnel ID for debugging

**Proxy Flow:**
```
Client → PreviewUrlProxy (TCP) → TunnelRegistry → kube-manager → K8s Service
         [Parse Host]            [Route to cluster]  [Forward]     [Handle]
```

### Access Control Levels

Defined in `/workspaces/lapdev/crates/common/src/kube.rs`:

| Level | Description | Authentication Required | Authorization Check |
|-------|-------------|------------------------|---------------------|
| **Personal** | Only accessible by owner | Yes | User must be owner |
| **Shared** | Accessible by org members | Yes | User must be in org |
| **Public** | Accessible by anyone | No | None |

Access validation happens in `PreviewUrlResolver::validate_access_level()` during URL resolution.

---

## Branch Environments Architecture

### Concept

Branch environments enable developers to:
- Create isolated environments from a shared "base" environment
- **Deploy only the workloads they need to modify**
- **Share all unmodified services with the base environment** (massive cost savings)
- Test changes independently before merging to shared environment

### Environment Types

| Type | `is_shared` | `base_environment_id` | Purpose |
|------|-------------|----------------------|---------|
| **Base/Shared** | `true` | `NULL` | Production or staging environment shared across org |
| **Personal** | `false` | `NULL` | Private environment owned by one user |
| **Branch** | `false` | UUID of base | Isolated dev environment forked from shared base |

**Rules:**
- Only **Shared** environments can be used as bases for branches
- Branch environments are always **Personal** (owned by creator)
- Cannot create a branch from another branch (no nested branching)

### Resource Sharing Model

**This is the key architectural innovation that saves costs:**

Branch environments **DO NOT** duplicate all services from the base environment. Instead:

1. **At Creation**: Only database records are created (no K8s resources)
2. **On First Modification**: Only the modified workload is deployed to K8s
3. **Unmodified Workloads**: Continue using the base environment's deployments
4. **Service Routing**: Preview URLs can route to either branch-specific or base workloads

**Example Workflow:**

```
Base Environment "staging" has 10 microservices:
  ├─ frontend (Deployment)
  ├─ api-gateway (Deployment)
  ├─ auth-service (Deployment)
  ├─ user-service (Deployment)
  ├─ payment-service (Deployment)
  ├─ notification-service (Deployment)
  ├─ analytics-service (Deployment)
  ├─ search-service (Deployment)
  ├─ cache (StatefulSet)
  └─ database (StatefulSet)

Developer creates branch "feature-login":
  1. Modifies auth-service (updates authentication logic)
  2. Modifies frontend (updates login UI)

  K8s Resources Deployed in Branch:
  ├─ auth-service-branch-{uuid} (Deployment) ← NEW
  └─ frontend-branch-{uuid} (Deployment) ← NEW

  Shared from Base (no deployment):
  ├─ api-gateway ← uses base environment's deployment
  ├─ user-service ← uses base environment's deployment
  ├─ payment-service ← uses base environment's deployment
  ├─ notification-service ← uses base environment's deployment
  ├─ analytics-service ← uses base environment's deployment
  ├─ search-service ← uses base environment's deployment
  ├─ cache ← uses base environment's deployment
  └─ database ← uses base environment's deployment

Cost: 2 deployments instead of 10 (80% reduction)
```

### Branch Environment Creation Flow

**Function:** `create_branch_environment()` in `/workspaces/lapdev/crates/api/src/kube_controller.rs`

**Steps:**

1. **Validation**
   ```
   - Verify base environment exists
   - Verify base belongs to same organization
   - Verify base.is_shared == true
   - Verify base.base_environment_id IS NULL
   - Verify cluster allows personal deployments
   ```

2. **Copy Configuration (Database Only)**
   ```
   - Retrieve all workloads from base environment
   - Retrieve all services from base environment
   - Convert to KubeWorkloadDetails and KubeServiceWithYaml
   ```

3. **Image Inheritance**
   ```
   For each container in workloads:
     - If base has custom image:
       → Set as original_image for branch
       → Set container image to FollowOriginal
     - Clear customized env vars
     - Preserve original_env_vars
   ```

4. **Create Database Records (No K8s Deployment)**
   ```
   Create kube_environment:
     - is_shared = false (always)
     - base_environment_id = <base env UUID>
     - namespace = <same as base>
     - app_catalog_id = <same as base>
     - cluster_id = <same as base>
     - name = <user-provided unique name>

   Copy all workloads and services with new environment_id

   ⚠️ NO Kubernetes resources created yet!
   ```

5. **Lazy Deployment on First Modification**
   - Branch environments exist only in database until workloads are modified
   - First workload update triggers K8s deployment
   - Saves cluster resources for unused branches

### Branch vs Base Workload Deployment

When updating a workload in an environment:

**Base Environment** (`base_environment_id IS NULL`):
```
- RPC: update_workload_containers()
- Action: Update existing K8s deployment in-place
- Deployment name: {workload_name}
- Labels: lapdev.environment={env_name}
```

**Branch Environment** (`base_environment_id IS NOT NULL`):
```
- RPC: create_branch_workload()
- Action: Create NEW K8s deployment (only for this workload)
- Deployment name: {workload_name}-branch-{env_id}
- Labels: lapdev.environment={branch_env_name}
- Selector: Unique to avoid conflicts with base
```

**File:** `/workspaces/lapdev/crates/api/src/kube_controller.rs:1644-1793`

### Namespace and Resource Isolation

Branch environments **share the same Kubernetes namespace** as their base environment but achieve isolation through:

1. **Unique Deployment Names**
   - Base: `deployment/webapp`
   - Branch: `deployment/webapp-branch-{uuid}`

2. **Unique Selectors**
   - Each branch deployment has unique labels
   - Services route only to their specific deployment

3. **Independent Lifecycle**
   - Deleting branch doesn't affect base
   - Deleting base doesn't auto-delete branches (orphan handling TBD)

**Example in namespace "staging":**
```yaml
# Base environment - all 10 services running
apiVersion: apps/v1
kind: Deployment
metadata:
  name: auth-service
  namespace: staging
  labels:
    lapdev.environment: staging

---

# Branch environment - only modified services deployed
apiVersion: apps/v1
kind: Deployment
metadata:
  name: auth-service-branch-abc123
  namespace: staging
  labels:
    lapdev.environment: feature-login

# Note: Other 9 services are NOT deployed in branch
# Branch environment's preview URLs can route to base services when needed
```

---

## Cost Optimization Through Resource Sharing

### The Problem with Traditional Isolated Environments

**Traditional Approach: Full Namespace Isolation**
```
Production Environment (namespace: production)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)

Staging Environment (namespace: staging)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)

Developer 1 - Feature Branch (namespace: feature-login)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)

Developer 2 - Feature Branch (namespace: feature-payment)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)

Developer 3 - Feature Branch (namespace: bugfix-auth)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)

Total: 75 GB RAM
Cost: 5× the production environment cost
```

**Issues:**
- Most services in branch environments are unchanged from staging
- Developers only modify 1-2 services but pay for all 10
- Cluster capacity wasted on duplicate deployments
- Slow environment spin-up (must deploy everything)
- Higher cloud costs (more nodes needed)

### Lapdev's Solution: Selective Deployment with Resource Sharing

```
Production Environment (namespace: production)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)

Staging Environment (namespace: staging)
  - All services deployed (10 services × 1.5 GB = 15 GB RAM)
  ← Base environment for branches

Developer 1 - Branch Environment (shares namespace: staging)
  - Only deploys: auth-service (512 MB)
  - Shares: 9 other services with staging

Developer 2 - Branch Environment (shares namespace: staging)
  - Only deploys: payment-service (1 GB)
  - Shares: 9 other services with staging

Developer 3 - Branch Environment (shares namespace: staging)
  - Only deploys: auth-service (512 MB) + frontend (1 GB)
  - Shares: 8 other services with staging

Total: 30 GB + 512 MB + 1 GB + 1.5 GB = 33 GB RAM
Cost: 2.2× production (vs 5× with traditional approach)
Savings: 56% reduction in infrastructure costs
```

### Cost Comparison Table

| Scenario | Traditional (Isolated NS) | Lapdev (Shared NS) | Savings |
|----------|--------------------------|-------------------|---------|
| **1 developer, 1 service modified** | 15 GB | 1.5 GB | 90% |
| **3 developers, avg 2 services modified** | 45 GB | 9 GB | 80% |
| **10 developers, avg 2 services modified** | 150 GB | 30 GB | 80% |
| **Short-lived test branch (no modifications)** | 15 GB | 0 GB | 100% |

### How It Works: Service Routing in Branch Environments

When a branch environment receives traffic via a preview URL:

1. **Modified Services**: Route to branch-specific deployment
   ```
   Preview URL: https://auth-8080-xyz.app.lap.dev
   → Resolves to: auth-service-branch-abc123.staging.svc.cluster.local:8080
   ```

2. **Unmodified Services**: Route to base environment deployment
   ```
   Preview URL: https://payment-9000-xyz.app.lap.dev
   → Resolves to: payment-service.staging.svc.cluster.local:9000
   ← Uses base environment's deployment (no branch deployment exists)
   ```

**Implementation Detail:**

Currently, when a branch environment is created, services are copied to the branch's database records but NOT deployed to K8s. Preview URLs created for unmodified services will route to the base environment's K8s Service, which points to the base deployment.

**Future Enhancement:**

Track which workloads have been deployed in branches and automatically route to base for undeployed workloads. This could be done by:
- Adding `deployed_to_k8s` boolean flag to `kube_environment_workload` table
- Preview URL resolver checks this flag
- If false, route to base environment's service instead

### Developer Experience

**Creating a branch environment is instant:**
```bash
# Create branch (no K8s resources deployed yet)
$ lapdev env create-branch --from staging --name feature-login
✓ Branch environment created in 0.5s
✓ Cost: $0/hour (no resources deployed)

# Modify auth-service
$ lapdev env update feature-login --workload auth-service --image myregistry/auth:feature-login
✓ Deploying auth-service to K8s...
✓ Deployed in 10s
✓ Cost: $0.02/hour (1 service × 512 MB)

# Create preview URL
$ lapdev preview create feature-login --service auth-service --port 8080
✓ https://auth-8080-xyz789.app.lap.dev

# Test your changes (all other services use staging environment)
```

**Comparing Costs:**

| Action | Traditional Isolated NS | Lapdev Shared NS |
|--------|------------------------|-----------------|
| Create environment | 60s (deploy all services) | 0.5s (DB only) |
| Cost while idle | $0.15/hour (all services running) | $0/hour (nothing deployed) |
| Cost after 1 modification | $0.15/hour (all services) | $0.02/hour (1 service) |
| Cost after 2 modifications | $0.15/hour (all services) | $0.04/hour (2 services) |
| Delete environment | 30s (delete all K8s resources) | 1s (soft delete in DB) |

### When to Use Each Approach

**Use Branch Environments (Shared Namespace) When:**
- ✅ Modifying 1-3 services in a large microservices architecture
- ✅ Short-lived feature development
- ✅ Testing small changes before merging to staging
- ✅ Cost optimization is important
- ✅ Services can share network policies

**Use Full Isolated Namespace When:**
- ❌ Modifying most/all services (>50% of workloads)
- ❌ Testing major infrastructure changes (network policies, RBAC)
- ❌ Strict security isolation required (compliance, multi-tenancy)
- ❌ Different resource quotas needed
- ❌ Long-lived environments (months)

**Best Practice:**
- Use **shared environments** as staging/integration bases
- Use **branch environments** for feature development (shares with staging)
- Use **isolated namespaces** for long-lived QA/UAT environments

---

## Database Schema

### `kube_environment`

Stores environment metadata.

**File:** `/workspaces/lapdev/crates/db/migration/src/m20250809_000001_create_kube_environment.rs`

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `id` | UUID | PRIMARY KEY | Unique environment identifier |
| `organization_id` | UUID | NOT NULL, FK | Organization ownership |
| `user_id` | UUID | NOT NULL, FK | Creator/owner |
| `app_catalog_id` | UUID | NOT NULL, FK | Source app catalog template |
| `cluster_id` | UUID | NOT NULL, FK | Target Kubernetes cluster |
| `name` | String | NOT NULL | Display name |
| `namespace` | String | NOT NULL | Kubernetes namespace |
| `status` | String | NOT NULL | Running/Pending/Failed/etc |
| `is_shared` | Boolean | NOT NULL | Shared vs personal environment |
| `base_environment_id` | UUID | NULLABLE, FK → self | For branch environments |
| `created_at` | Timestamp | NOT NULL | Creation time |
| `deleted_at` | Timestamp | NULLABLE | Soft delete timestamp |

**Constraints:**
- Only one base environment per `(app_catalog_id, cluster_id, namespace)` where `base_environment_id IS NULL`

**Indexes:**
- `organization_id`
- `cluster_id`
- `base_environment_id`

### `kube_environment_preview_url`

Stores preview URL configurations.

**File:** `/workspaces/lapdev/crates/db/migration/src/m20250815_000001_create_kube_environment_preview_url.rs`

| Column | Type | Constraints | Description |
|--------|------|-------------|-------------|
| `id` | UUID | PRIMARY KEY | Unique preview URL identifier |
| `environment_id` | UUID | NOT NULL, FK | Parent environment |
| `service_id` | UUID | NOT NULL, FK | Target service |
| `name` | String | NOT NULL, UNIQUE | Subdomain identifier |
| `description` | Text | NULLABLE | User-provided description |
| `port` | i32 | NOT NULL | Target service port |
| `port_name` | String | NULLABLE | Named port reference |
| `protocol` | String | NOT NULL | "HTTP", "HTTPS", "WebSocket" |
| `access_level` | String | NOT NULL | "personal", "shared", "public" |
| `created_by` | UUID | NOT NULL, FK | User who created URL |
| `last_accessed_at` | Timestamp | NULLABLE | For analytics/cleanup |
| `metadata` | JSON | NULLABLE | Custom headers, rate limits, etc |
| `created_at` | Timestamp | NOT NULL | Creation time |
| `deleted_at` | Timestamp | NULLABLE | Soft delete timestamp |

**Constraints:**
- `name` UNIQUE (global subdomain uniqueness)
- `(service_id, port)` UNIQUE (one preview URL per service port)

**Indexes:**
- `environment_id`
- `service_id`
- `name` (for fast resolution)

### `kube_environment_workload`

Stores workload definitions (Deployments, StatefulSets, etc).

**File:** `/workspaces/lapdev/crates/db/migration/src/m20250809_000002_create_kube_environment_workload.rs`

| Column | Type | Description |
|--------|------|-------------|
| `id` | UUID | Unique workload identifier |
| `environment_id` | UUID | Parent environment |
| `name` | String | Workload name |
| `namespace` | String | Kubernetes namespace |
| `kind` | String | "Deployment", "StatefulSet", etc |
| `containers` | JSON | Array of `KubeContainerInfo` |
| `created_at` | Timestamp | Creation time |
| `deleted_at` | Timestamp | Soft delete |

**Container JSON Structure:**
```json
{
  "name": "webapp",
  "image": "Custom",
  "custom_image": "myregistry/webapp:v2.0",
  "original_image": "myregistry/webapp:v1.0",
  "resources": {...},
  "env_vars": {...},
  "original_env_vars": {...}
}
```

### `kube_environment_service`

Stores Kubernetes Service definitions.

**File:** `/workspaces/lapdev/crates/db/migration/src/m20250809_000003_create_kube_environment_service.rs`

| Column | Type | Description |
|--------|------|-------------|
| `id` | UUID | Unique service identifier |
| `environment_id` | UUID | Parent environment |
| `name` | String | Service name |
| `namespace` | String | Kubernetes namespace |
| `yaml` | Text | Full YAML definition |
| `ports` | JSON | Array of service ports |
| `selector` | JSON | Pod selector labels |
| `created_at` | Timestamp | Creation time |
| `deleted_at` | Timestamp | Soft delete |

---

## Request Flow Diagrams

### Preview URL Access Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│ 1. User Request                                                     │
│    GET https://webapp-8080-abc123.app.lap.dev/api/users            │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 2. PreviewUrlProxy (TCP Server)                                     │
│    - Accept TCP connection                                          │
│    - Parse HTTP headers                                             │
│    - Extract Host: webapp-8080-abc123.app.lap.dev                  │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 3. PreviewUrlResolver                                               │
│    - Parse subdomain: service="webapp", port=8080, hash="abc123"   │
│    - Query kube_environment WHERE name LIKE '%abc123%'             │
│    - Query kube_environment_service WHERE name='webapp'            │
│    - Query kube_environment_preview_url WHERE port=8080            │
│    - Validate access level                                          │
│    - Return PreviewUrlTarget                                        │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 4. Add Tracestate Header                                            │
│    tracestate: lapdev-env-id={environment_id}                       │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 5. TunnelRegistry                                                   │
│    - Find tunnel to target cluster                                  │
│    - Send tunnel message with target service info                   │
│    - Open bidirectional TCP stream                                  │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 6. kube-manager (in cluster)                                        │
│    - Receive tunnel request                                         │
│    - Connect to K8s service: webapp.staging.svc.cluster.local:8080 │
│    - Forward HTTP request                                           │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 7. Kubernetes Service → Pod                                         │
│    - Route to healthy pod                                           │
│    - Application processes request                                  │
│    - Return response                                                │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 8. Response Stream                                                  │
│    Pod → kube-manager → Tunnel → PreviewUrlProxy → Client          │
└─────────────────────────────────────────────────────────────────────┘
```

### Branch Environment Creation Flow

```
┌─────────────────────────────────────────────────────────────────────┐
│ 1. Shared Environment "staging" exists                              │
│    - is_shared = true                                               │
│    - base_environment_id = NULL                                     │
│    - Deployed to K8s namespace "staging"                            │
│    - Workloads: webapp, api, auth, payment, notifications, etc     │
│    - Total: 10 services, 15 GB RAM                                  │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 2. Developer Creates Branch "feature-login"                         │
│    POST /api/kube/environment/branch                                │
│    {                                                                │
│      "base_environment_id": "staging-uuid",                         │
│      "name": "feature-login"                                        │
│    }                                                                │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 3. Validation                                                       │
│    ✓ Base environment exists                                        │
│    ✓ Base is shared (is_shared=true)                               │
│    ✓ Base is not itself a branch (base_environment_id=NULL)        │
│    ✓ User belongs to same organization                             │
│    ✓ Cluster allows personal deployments                           │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 4. Copy Configuration to Database ONLY                              │
│                                                                     │
│    Create kube_environment:                                         │
│      - id = new-uuid                                                │
│      - name = "feature-login"                                       │
│      - base_environment_id = "staging-uuid"                         │
│      - is_shared = false                                            │
│      - namespace = "staging" (same as base)                         │
│                                                                     │
│    Copy 10 workload definitions (DB only)                           │
│    Copy 10 service definitions (DB only)                            │
│                                                                     │
│    💰 Cost at this point: $0/hour (no K8s resources)               │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 5. NO Kubernetes Deployment Yet                                     │
│    - Branch environment exists only in database                     │
│    - No K8s resources created                                       │
│    - Preview URLs would route to base environment services          │
│    - Waiting for first workload update                              │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 6. Developer Updates 2 Workloads                                    │
│    PATCH /api/kube/environment/{id}/workload/auth-service          │
│    PATCH /api/kube/environment/{id}/workload/frontend              │
│                                                                     │
│    Changes:                                                         │
│    - auth-service: custom image for new login flow                 │
│    - frontend: custom image for new UI                             │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 7. Deploy ONLY Modified Workloads to Kubernetes                     │
│                                                                     │
│    RPC: create_branch_workload() × 2                                │
│                                                                     │
│    Creates in namespace "staging":                                  │
│      1. Deployment: auth-service-branch-{uuid} (512 MB)            │
│      2. Deployment: frontend-branch-{uuid} (1 GB)                  │
│                                                                     │
│    NOT deployed (uses base environment):                            │
│      - api, payment, notifications, analytics, search, cache, db   │
│                                                                     │
│    💰 Cost: $0.05/hour (2 services) vs $0.15/hour (10 services)   │
│    💰 Savings: 67% reduction                                        │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 8. Create Preview URLs                                              │
│    POST /api/kube/environment/{id}/preview-url                     │
│                                                                     │
│    Modified services (route to branch):                             │
│    - https://auth-8080-xyz.app.lap.dev → auth-service-branch-{uuid}│
│    - https://frontend-3000-abc.app.lap.dev → frontend-branch-{uuid}│
│                                                                     │
│    Unmodified services (route to base):                             │
│    - https://payment-9000-def.app.lap.dev → payment-service (base) │
│    - All other services route to base environment                   │
└────────────────────────────────┬────────────────────────────────────┘
                                 │
                                 ▼
┌─────────────────────────────────────────────────────────────────────┐
│ 9. Developer Tests Changes                                          │
│    - Modified auth + frontend: Uses branch deployments             │
│    - Unmodified 8 services: Uses base deployments                   │
│    - Changes isolated from "staging" for modified services          │
│    - Shares infrastructure for unchanged services                   │
│    - Can iterate without affecting team or paying for full stack    │
└─────────────────────────────────────────────────────────────────────┘
```

---

## Security and Access Control

### Organization-Level Security

All environments belong to an organization:
- User must be authenticated
- User must be member of the organization
- Verified by `organization_user` table join

**Implementation:** All API endpoints check organization membership.

### Environment-Level Access

| Environment Type | Who Can Access | Check Method |
|------------------|----------------|--------------|
| Shared | Any org member | Check `is_shared=true` + org membership |
| Personal | Owner only | Check `user_id = requesting_user` |
| Branch | Creator only | Check `user_id = requesting_user` (always personal) |

**Implementation:** `/workspaces/lapdev/crates/api/src/kube_controller.rs`

### Preview URL Access Control

Three-tier access model:

**1. Personal**
- Requires authentication
- User must be the environment owner (`environment.user_id = user.id`)
- Most restrictive

**2. Shared**
- Requires authentication
- User must be in the same organization (`environment.organization_id = user.organization_id`)
- Medium security

**3. Public**
- No authentication required
- Anyone can access
- Least restrictive (use with caution)

**Access Validation Flow:**
```rust
fn validate_access_level(
    preview_url: &PreviewUrl,
    environment: &Environment,
    user: Option<&User>
) -> Result<()> {
    match preview_url.access_level {
        AccessLevel::Public => Ok(()),
        AccessLevel::Shared => {
            let user = user.ok_or(Unauthorized)?;
            if user.organization_id == environment.organization_id {
                Ok(())
            } else {
                Err(Forbidden)
            }
        }
        AccessLevel::Personal => {
            let user = user.ok_or(Unauthorized)?;
            if user.id == environment.user_id {
                Ok(())
            } else {
                Err(Forbidden)
            }
        }
    }
}
```

**Implementation:** `/workspaces/lapdev/crates/kube/src/preview_url.rs`

### Security Considerations

**Risks:**
- Public preview URLs expose services without authentication
- Branch environments in shared namespace could access base environment's secrets
- Subdomain enumeration could reveal organization structure

**Mitigations:**
- Access level defaults to "Personal" (most restrictive)
- Network policies should isolate branch deployments
- Rate limiting on preview URL creation (TODO: implement)
- Audit logging for all preview URL access (via `last_accessed_at`)

---

## API Reference

### Environment Operations

**List Environments**
```
GET /api/kube/environments
Query Params:
  - type: "personal" | "shared" | "branch"

Response: Array<{
  id: UUID,
  name: String,
  namespace: String,
  cluster_id: UUID,
  is_shared: Boolean,
  base_environment_id?: UUID,
  status: String
}>
```

**Get Environment Details**
```
GET /api/kube/environment/{id}

Response: {
  id: UUID,
  name: String,
  namespace: String,
  cluster_id: UUID,
  is_shared: Boolean,
  base_environment_id?: UUID,
  workloads: Array<Workload>,
  services: Array<Service>,
  preview_urls: Array<PreviewUrl>
}
```

**Create Base/Personal Environment**
```
POST /api/kube/environment

Body: {
  name: String,
  app_catalog_id: UUID,
  cluster_id: UUID,
  namespace: String,
  is_shared: Boolean
}

Response: {
  id: UUID,
  ...
}
```

**Create Branch Environment**
```
POST /api/kube/environment/branch

Body: {
  base_environment_id: UUID,
  name: String
}

Response: {
  id: UUID,
  base_environment_id: UUID,
  ...
}
```

**Delete Environment**
```
DELETE /api/kube/environment/{id}

Response: 204 No Content
```

### Preview URL Operations

**List Preview URLs for Environment**
```
GET /api/kube/environment/{id}/preview-urls

Response: Array<{
  id: UUID,
  name: String,
  url: String,
  service_id: UUID,
  port: Integer,
  access_level: "personal" | "shared" | "public",
  description?: String
}>
```

**Create Preview URL**
```
POST /api/kube/environment/{id}/preview-url

Body: {
  service_id: UUID,
  port: Integer,
  access_level: "personal" | "shared" | "public",
  description?: String
}

Response: {
  id: UUID,
  name: String,
  url: String,
  ...
}
```

**Update Preview URL**
```
PATCH /api/kube/environment/{env_id}/preview-url/{url_id}

Body: {
  description?: String,
  access_level?: "personal" | "shared" | "public"
}

Response: {
  id: UUID,
  ...
}
```

**Delete Preview URL**
```
DELETE /api/kube/environment/{env_id}/preview-url/{url_id}

Response: 204 No Content
```

### Workload Operations

**List Environment Workloads**
```
GET /api/kube/environment/{id}/workloads

Response: Array<{
  id: UUID,
  name: String,
  kind: String,
  containers: Array<Container>
}>
```

**Update Workload**
```
PATCH /api/kube/environment/{id}/workload/{workload_id}

Body: {
  containers: Array<{
    name: String,
    image: "FollowOriginal" | "Custom",
    custom_image?: String,
    env_vars?: Object
  }>
}

Response: 200 OK
```

---

## Key Files Reference

### Backend Core

| File | Purpose |
|------|---------|
| `/workspaces/lapdev/crates/kube/src/preview_url.rs` | Preview URL parsing, resolution, and validation |
| `/workspaces/lapdev/crates/kube/src/http_proxy.rs` | TCP-based HTTP proxy for preview URLs |
| `/workspaces/lapdev/crates/api/src/kube_controller.rs` | Business logic for environments, workloads, preview URLs |
| `/workspaces/lapdev/crates/db/src/api.rs` | Database CRUD operations |

### Database Migrations

| File | Purpose |
|------|---------|
| `/workspaces/lapdev/crates/db/migration/src/m20250809_000001_create_kube_environment.rs` | Environment table |
| `/workspaces/lapdev/crates/db/migration/src/m20250809_000002_create_kube_environment_workload.rs` | Workload table |
| `/workspaces/lapdev/crates/db/migration/src/m20250809_000003_create_kube_environment_service.rs` | Service table |
| `/workspaces/lapdev/crates/db/migration/src/m20250815_000001_create_kube_environment_preview_url.rs` | Preview URL table |

### Database Entities

| File | Purpose |
|------|---------|
| `/workspaces/lapdev/crates/db/entities/src/kube_environment.rs` | Environment entity model |
| `/workspaces/lapdev/crates/db/entities/src/kube_environment_workload.rs` | Workload entity model |
| `/workspaces/lapdev/crates/db/entities/src/kube_environment_service.rs` | Service entity model |
| `/workspaces/lapdev/crates/db/entities/src/kube_environment_preview_url.rs` | Preview URL entity model |

### Frontend Components

| File | Purpose |
|------|---------|
| `/workspaces/lapdev/crates/dashboard/src/kube_environment.rs` | Environment list view (Personal/Shared/Branch tabs) |
| `/workspaces/lapdev/crates/dashboard/src/kube_environment_detail.rs` | Environment detail page with workloads/services/preview URLs |
| `/workspaces/lapdev/crates/dashboard/src/kube_environment_preview_url.rs` | Preview URL management UI |

### Common Types

| File | Purpose |
|------|---------|
| `/workspaces/lapdev/crates/common/src/kube.rs` | Shared data structures (AccessLevel, KubeWorkloadDetails, etc) |

---

## Design Decisions

### 1. Selective Deployment: Deploy Only What Changes

**Decision:** Branch environments only deploy workloads that are explicitly modified by developers. Unmodified workloads share the base environment's deployments.

**Rationale:**
- **Cost Savings:** Reduces infrastructure costs by 70-90% for typical feature branches
- **Faster Iteration:** No need to wait for full stack deployment
- **Resource Efficiency:** Maximizes cluster capacity by avoiding duplicate deployments
- **Developer Experience:** Instant branch creation (database only)

**Trade-offs:**
- Requires tracking which workloads are deployed in branches
- Preview URL routing is more complex (must handle both branch and base services)
- Potential confusion if developers expect full isolation

**Metrics:**
- Average branch uses 1-2 services (15% of total)
- Cost reduction: 85% per branch environment
- Deployment time: 0.5s (DB only) vs 60s (full stack)

**Alternative Considered:** Deploy all services immediately
- Rejected due to cost (5× infrastructure) and deployment time

### 2. Branch Environments Share Namespace with Base

**Decision:** Branch environments deploy to the same Kubernetes namespace as their base environment, using different deployment names for isolation.

**Rationale:**
- Simplifies network policies (services can communicate within namespace)
- Reduces cluster resource consumption (fewer namespaces to manage)
- Allows sharing of ConfigMaps and Secrets from base
- **Enables resource sharing model** (unmodified services use base deployments)

**Trade-offs:**
- Potential resource conflicts (PVCs, Services with same name)
- Less strict isolation (network policies apply at namespace level)
- Requires careful naming to avoid collisions

**Alternative Considered:** Create separate namespace per branch
- Rejected due to namespace proliferation, management overhead, and inability to share resources

### 3. Lazy Deployment of Branch Environments

**Decision:** Branch environments are created in the database immediately but K8s resources are not deployed until workloads are first modified.

**Rationale:**
- Saves cluster resources for unused branches
- Faster branch creation (no K8s API calls)
- Allows review of configuration before deployment
- **Zero cost for branches that are created but never used**

**Trade-offs:**
- Inconsistent state (database exists but K8s doesn't)
- Potential confusion for users (environment shows "Pending" until deployed)

**Alternative Considered:** Deploy immediately
- Rejected due to resource waste for short-lived test branches

### 4. TCP-Level HTTP Proxying

**Decision:** The preview URL proxy operates at the TCP level, not HTTP request/response parsing.

**Rationale:**
- Supports WebSocket upgrades seamlessly
- Supports HTTP/2 and streaming protocols
- Lower latency (no request buffering)
- Simpler implementation (no HTTP parser needed)

**Trade-offs:**
- Cannot modify response bodies
- Cannot implement request-level rate limiting
- Cannot add custom headers to responses (only requests)

**Alternative Considered:** Full HTTP proxy with request/response parsing
- Rejected due to complexity and WebSocket support requirements

### 5. Subdomain-Based Preview URLs

**Decision:** Preview URLs use subdomains with embedded metadata: `{service}-{port}-{hash}.app.lap.dev`

**Rationale:**
- Stateless resolution (no database lookup for routing)
- Globally unique (enforced by DNS)
- Human-readable (service and port visible)
- SEO-friendly (each URL is distinct)

**Trade-offs:**
- Exposes service names in URL
- Hash is not cryptographically secure (12 random chars)
- Cannot change URL after creation (name is immutable)

**Alternative Considered:** Path-based routing (`app.lap.dev/{hash}/...`)
- Rejected due to path prefix conflicts and less clean URLs

### 6. Image Inheritance from Base to Branch

**Decision:** When creating a branch, if the base has custom images, they are set as `original_image` in the branch and the container image is set to `FollowOriginal`.

**Rationale:**
- Allows developers to see what the base is running
- Provides clear diff between base and branch
- Enables easy revert to base configuration

**Trade-offs:**
- Requires two-step update (first to `FollowOriginal`, then to `Custom`)
- More complex data model (`original_image` vs `custom_image`)

**Alternative Considered:** Copy custom images as-is
- Rejected because branches would immediately diverge from base without visibility

### 7. Three-Tier Access Control

**Decision:** Preview URLs support three access levels: Personal, Shared, Public.

**Rationale:**
- Personal: Safe default for private development
- Shared: Enables team collaboration without exposing to internet
- Public: Necessary for stakeholder/client demos

**Trade-offs:**
- "Public" is dangerous if misused (exposed to internet)
- No fine-grained permissions (e.g., specific user allowlist)

**Alternative Considered:** Binary public/private
- Rejected because team collaboration is a core use case

### 8. Soft Deletes Throughout

**Decision:** All tables use `deleted_at` timestamp instead of hard deletes.

**Rationale:**
- Enables audit trail
- Allows undoing accidental deletions
- Historical analytics (e.g., preview URL access trends)

**Trade-offs:**
- Unique constraints must account for soft-deleted records
- Database grows over time (requires cleanup jobs)
- More complex queries (always filter `deleted_at IS NULL`)

**Alternative Considered:** Hard deletes with archive tables
- Rejected due to complexity of maintaining separate archive schema

### 9. No Nested Branching

**Decision:** Branch environments can only be created from shared environments, not from other branches.

**Rationale:**
- Simplifies mental model (two-level hierarchy: base → branches)
- Prevents complex dependency chains
- Reduces risk of orphaned environments
- **Cost control:** Prevents exponential resource growth from nested branches

**Trade-offs:**
- Cannot create sub-branches for experimental features
- Developers must merge to base before creating new branch

**Alternative Considered:** Allow nested branching
- Rejected due to UX complexity, unclear use case, and cost explosion risk

---

## Future Enhancements

### Planned
- [ ] Dedicated `environment_hash` field (not reusing `name`)
- [ ] Rate limiting for preview URL creation
- [ ] Preview URL analytics dashboard
- [ ] Network policies for branch environment isolation
- [ ] Automatic cleanup of stale branch environments
- [ ] Branch environment templates (pre-configured workload overrides)
- [ ] Cost estimation API (show cost before creating branch)
- [ ] Workload deployment tracking (`deployed_to_k8s` flag)

### Under Consideration
- [ ] Custom domain support for preview URLs
- [ ] Fine-grained access control (user/team allowlists)
- [ ] Preview URL expiration dates
- [ ] Branch environment diffing (show changes from base)
- [ ] Automatic branch environment creation from git hooks
- [ ] Resource quotas per developer (max N concurrent branches)
- [ ] Cost reporting dashboard (per environment, per developer, per org)

---

## Glossary

| Term | Definition |
|------|------------|
| **Base Environment** | A shared environment that can be used as the source for branch environments |
| **Branch Environment** | A personal environment forked from a shared base environment, deploying only modified workloads |
| **Preview URL** | A publicly accessible URL that routes to a service in an environment |
| **Access Level** | Security tier for preview URLs: Personal, Shared, or Public |
| **Environment** | A collection of Kubernetes workloads and services deployed to a cluster |
| **Workload** | A Kubernetes resource that runs containers (Deployment, StatefulSet, etc) |
| **Service** | A Kubernetes Service that exposes workload pods |
| **App Catalog** | A template defining workloads and services that can be deployed |
| **Tunnel** | A persistent TCP connection between lapdev-api and kube-manager for proxying |
| **Selective Deployment** | Branch environment strategy: deploy only modified workloads, share the rest |
| **Resource Sharing** | Branch environments using base environment deployments for unmodified services |

---

## Cost Optimization Summary

**Key Takeaway:** Lapdev's branch environment architecture achieves **70-90% cost reduction** compared to traditional isolated namespace approaches by deploying only the workloads that developers modify.

**How It Works:**
1. Create branch environment (database only) → $0/hour
2. Modify 1-2 services → Deploy only those services
3. Share remaining 8-9 services with base environment
4. Result: Pay for 2 services instead of 10

**Typical Savings:**
- 1 developer, 2 modified services: **87% cost reduction**
- 5 developers, avg 2 modified services: **80% cost reduction**
- 10 developers, avg 2 modified services: **80% cost reduction**

**Best For:**
- Microservices architectures (>5 services)
- Feature branch development
- Short-lived test environments
- Cost-conscious organizations

---

**Document Version:** 1.0
**Last Updated:** 2025-10-01
**Maintainers:** Lapdev Core Team
