# Automatic Environment Sync Feature Design

**Status:** Approved
**Date:** 2025-10-17

## Overview

The automatic sync feature monitors production Kubernetes manifests for changes and propagates those changes to App Catalogs and Environments. This ensures development environments stay up-to-date with production without manual intervention.

## Problem Statement

Currently, when production workloads change (new container images, updated ConfigMaps, resource limit changes), these changes don't automatically propagate to:
1. App Catalogs (the blueprint)
2. Development Environments (running instances)

This causes drift between production and development environments.

## Goals
- Keep Lapdev catalogs and environments aligned with production-derived manifests with minimal human interaction.
- Give teams fine-grained control over when and how changes propagate so active development work is not disrupted.
- Centralize business logic in the API server to avoid customer-side upgrades when iteration is needed.
- Provide the minimum viable visibility (counts, names, timestamps) required for users to trust automated propagation.
- Lay groundwork for richer change insight (diffs, approvals, policy) without blocking the first release.

## Non-Goals
- Building a full GitOps replacement or storing full manifest history beyond what is needed for sync decisions.
- Surfacing full YAML diffs or per-field approvals in the initial release.
- Automatically reconciling environment-specific overrides (resource patches, image overrides) created by developers.
- Handling cross-cluster topologies; v1 is scoped to a single source cluster feeding multiple Lapdev catalogs/environments.

## Assumptions & Scope
- Production clusters already expose the namespaces we watch and can be reached by kube-manager.
- Catalog workloads accurately enumerate the production workloads we care about; unexpected workloads are ignored.
- Environment namespaces may contain additional developer resources; only catalog-owned workloads and discovered dependencies are mutated.
- Secrets stay below size limits that make in-band transport feasible; large-object handling is deferred to future work.

## Vocabulary
- **Source Cluster**: The production Kubernetes cluster we mirror from.
- **App Catalog**: The Lapdev blueprint that defines which workloads belong to a product line.
- **Environment**: A Lapdev-managed namespace (shared or branch) created from a catalog.
- **Auto-Sync**: Flag indicating the system may apply changes without waiting for explicit approval at that level.
- **Sync Record**: Row in `kube_app_catalog_sync_status` that tracks detected workload changes.

## Architecture Decision: Catalog vs Environment Storage

**Key Design Choice:**

- **Workloads**: Stored in `kube_app_catalog_workload` (catalog-level)
  - Catalogs define which workloads to track
  - Environments inherit workload specs from catalog

- **ConfigMaps, Secrets, Services**: Stored in `kube_environment_*` tables (environment-level only)
  - NOT stored in catalog tables
  - Catalog watches detect changes, but data lives on environments
  - Each environment has its own ConfigMaps/Secrets/Services
  - Sync updates environment-level resources directly

**Rationale:**
- ConfigMaps/Secrets often contain environment-specific values (dev vs staging vs prod)
- Services are discovered dynamically based on current workload selectors
- Storing at environment level allows per-environment customization while still syncing from source
- Catalogs serve as blueprints that update automatically when source workloads change, while still allowing teams to curate manual edits when required.
- A dedicated dependency index (`kube_environment_dependency`) tracks which production ConfigMaps/Secrets feed each environment so change detection can fan out events without re-scanning workloads.

## Design Decisions

### 1. Change Detection: Kubernetes Watch

**Decision:** Use Kubernetes Watch API for real-time change detection

**Implementation Architecture:**

**Kube-Manager (Deployed in Customer Cluster):**
- Simple event forwarder with minimal logic
- Takes list of namespaces to watch (provided by API server)
- Watches **ALL** resource types in those namespaces:
  - Workloads: Deployments, StatefulSets, DaemonSets, ReplicaSets, Jobs, CronJobs
  - ConfigMaps
  - Secrets
  - Services
- Sends raw change events to API server
- **No decision logic** - just forwards events

**API Server (Centralized Lapdev Service):**
- Receives raw change events from kube-manager
- **Intelligent decision making:**
  - Which events belong to which catalogs?
  - Which workload changes should trigger catalog sync?
  - Which ConfigMap/Secret/Service changes should trigger environment sync?
  - Should this change trigger immediate downstream syncs or surface manual prompts?
- Can be improved/upgraded without touching customer deployments

**Rationale:**
- Kube-manager is deployed in customer clusters (hard to upgrade)
- API server is centralized (easy to upgrade/improve)
- Separation of concerns: kube-manager = event collector, API = decision maker
- Business logic changes don't require customer software upgrades

**RPC Interface:**

```rust
// Kube-Manager → API Server (reports events)
trait KubeClusterRpc {
    async fn report_resource_change(event: ResourceChangeEvent) -> Result<()>;
}

struct ResourceChangeEvent {
    namespace: String,
    resource_type: ResourceType, // Deployment, StatefulSet, DaemonSet, ConfigMap, Secret, Service
    resource_name: String,
    change_type: ChangeType, // Created, Updated, Deleted
    resource_yaml: String, // Full resource spec in YAML format
    timestamp: DateTime<Utc>,
}

// API Server → Kube-Manager (configures watches)
trait KubeManagerRpc {
    async fn configure_watches(namespaces: Vec<String>) -> Result<()>;
}
```

**Event Flow Example:**
1. Production deployment `api-server` updated in namespace `production`
2. Kube-manager detects change via Kubernetes Watch
3. Kube-manager sends: `report_resource_change(ResourceChangeEvent { namespace: "production", resource_type: Deployment, resource_name: "api-server", ... })`
4. API server receives event
5. API server queries: "Which catalog has workload 'api-server' in namespace 'production'?" → finds catalog ID
6. API server fetches the latest workload manifest from the source cluster, updates the catalog workload spec, and records an `auto_applied` activity (actor `NULL`).
7. Environments referencing the catalog have their `latest_catalog_sync_id` set to the new activity record and receive notifications prompting `Sync From Catalog` / `Sync With Cluster`.

### 2. Catalog Updates: Auto From Cluster & Manual Edits

**Decision:** Catalog specs update automatically when the reference workloads in the source cluster change, and administrators can still make explicit edits through the UI.

**Behavior (Cluster-Driven Auto Update):**
- Kube-manager events deliver the latest workload manifest to the API server.
- Catalog workload specs are updated in place, and an `auto_applied` activity is recorded (`actor_id = NULL`).
- Environments referencing the catalog set `latest_catalog_sync_id` to the new activity and proceed through the environment sync flow (auto or manual).

**Behavior (Admin Edits):**
- When a user saves catalog edits, the platform applies the new workload set immediately and records an `applied` activity with the actor’s ID.
- These manual edits follow the same environment sync flow, respecting environment auto-sync flags.

**Catalog Activity Summary (Auto Update Example):**

```
┌─────────────────────────────────────────────────┐
│ App Catalog: production-services                │
├─────────────────────────────────────────────────┤
│ ℹ️  Production change auto-applied               │
│                                                  │
│ Workloads updated:                              │
│   • api-server                                   │
│   • worker                                       │
│ Applied at: 2025-10-17T22:15Z                   │
│                                                  │
│ [Review Workloads]                              │
└─────────────────────────────────────────────────┘
```

When a user edits the catalog, the summary shifts to reflect the applied change (e.g., "✅ Workload 'api-server' added by Alice") and references the same activity log for auditing.

**Scope:**
- **App Catalog watches**: Workloads only (for notification context)
- **No tracking**: ConfigMaps, Secrets, Services (environment-level concerns)
- **Granularity**: Workload counts/names sufficient for review
- **No detailed diffs**: Too complex for v1; link routes to workload list for context

### 3. Environment Sync: Default Manual, Auto Opt-In

**Decision:** Environment sync defaults to manual approval, but environments can opt into auto-sync

**Rationale:**
- Developers are actively working in environments; unexpected updates can break work in progress.
- Some teams want fully automated drift correction for shared QA/staging namespaces.
- Allowing an explicit opt-in keeps developers in control while supporting hands-off pipelines.

**Behavior:**
- When a user updates the catalog (add/remove workloads), ALL environments show "Catalog update available" with `Sync From Catalog` highlighted.
- When ConfigMap/Secret changes arrive from production, impacted environments show "Config update available" with `Sync With Cluster` highlighted.
- Environments with `auto_sync=false` require a manual button click for each action type; auto environments run both actions as events arrive.
- Environments with `auto_sync=true` apply the relevant action automatically and surface the outcome afterward (success, failure, conflict).

**Environment Notification (Simple Summary):**
When catalog updates, environments are notified with a **simple summary**:

```
┌─────────────────────────────────────────────────┐
│ Environment: alice-dev                          │
├─────────────────────────────────────────────────┤
│ ⚠️  Update Available                            │
│                                                  │
│ Catalog 'production-services' has been updated: │
│   • 3 Workloads changed                         │
│                                                  │
│ [Sync From Catalog] [Sync With Cluster] [Dismiss]│
└─────────────────────────────────────────────────┘
```

`Dismiss` records the prompt as dismissed in `kube_environment_dependency_sync_status` (for dependency events) or emits an audit log event for catalog prompts (no schema change) without mutating workloads, ensuring auditability while letting developers defer action.

**Environment Sync Actions:**
- **`Sync From Catalog`** (workload-focused)
  1. Apply catalog workload spec changes, including adds/removes.
  2. Reconcile workload-level metadata (labels, annotations, autoscaling) to match the catalog.
  3. Trigger dependency discovery for any new/removed workloads and update `kube_environment_dependency` accordingly.
  4. Update environment tables for workloads plus referenced ConfigMaps/Secrets/Services encountered during discovery.
- **`Sync With Cluster`** (dependency-focused)
  1. Rediscover ConfigMaps, Secrets, and Services from the source cluster based on existing dependency index rows.
  2. Refresh values in `kube_environment_configmap`, `kube_environment_secret`, `kube_environment_service`.
  3. Update dependency index discovery timestamps; remove rows whose backing resources no longer exist.
  4. Leave workloads untouched, allowing developers to defer catalog changes while still pulling latest configs.

**Manual Sync Confirmation (Sync From Catalog):**
1. UI requests latest catalog sync metadata and displays confirmation dialog with timestamp + workload count.
2. Backend verifies the catalog sync referenced by the environment (`latest_catalog_sync_id`) is the newest entry with `status` in (`auto_applied`,`applied`); if not, the UI refreshes with the latest summary.
3. Environment Sync Orchestrator enqueues a reconciliation job marked `manual_trigger=true`, `action='catalog'`.
4. Job locks the environment record (optimistic concurrency) to prevent overlapping catalog syncs.
5. Job runs the workload reconciliation pipeline (catalog apply → dependency discovery → service refresh).
6. Upon success, the environment’s `last_catalog_synced_at` and `last_dependency_synced_at` are updated and a success notification is pushed to the user; on failure, the error message is captured and surfaced with retry option.
7. Audit trail records the initiating user, catalog sync id, action, and outcome to support compliance and troubleshooting.

**Manual Sync Confirmation (Sync With Cluster):**
1. UI fetches dependency event summary from `kube_environment_dependency_sync_status` (list of ConfigMaps/Secrets flagged by API).
2. Backend ensures no newer `kube_environment_dependency_sync_status` (via `latest_dependency_sync_id`) has superseded the request; if found, UI refreshes the summary.
3. Orchestrator enqueues reconciliation job with `manual_trigger=true`, `action='cluster'`.
4. Job locks the environment dependency context to avoid racing with catalog or auto dependency syncs.
5. Job executes dependency refresh pipeline only (config/secret/service discovery) and updates `kube_environment_dependency`.
6. On success, the environment’s `last_dependency_synced_at` is updated; failures are recorded with actionable error payloads.
7. Audit trail captures user, dependency sync status id, action, and outcome.

**Granularity:**
- ✅ Show: "3 workloads changed" (from catalog)
- For catalog prompts, omit ConfigMap/Secret/Service counts (discovered during follow-up)
- ❌ No detailed diffs (too complex)
- For dependency prompts, show count per resource type (e.g., "2 ConfigMaps", "1 Secret") sourced from `kube_environment_dependency_sync_status` summaries.

### 4. Branch Environment Handling: Sync Both

**Decision:** Update both shared and branch environments

**Behavior:**
- Shared environments: Always sync when catalog updates
- Branch environments: Sync branched workloads if they match catalog workloads
- Preserves branch-specific modifications (custom images, env overrides remain untouched unless explicitly in catalog)

## Two-Level Sync Control

### Notification Granularity

The system uses **simple notifications at both levels**:

**1. Catalog Level:**
- Shows workload count only: `3 Workloads changed`
- No detailed field-level diffs
- **Purpose:** Quick confirmation - "production workloads changed, catalog updated."

**2. Environment Level:**
- Shows workload count only: `3 Workloads changed`
- No ConfigMap/Secret/Service info (discovered during sync)
- **Purpose:** Simple decision - "catalog updated, sync environment?"

**What happens during sync:**
- Catalog sync: Update workload specs in `kube_app_catalog_workload`
- Environment sync: Update workloads + rediscover ConfigMaps/Secrets/Services from production

### Auto-Sync Control

Catalogs update automatically on cluster changes and can also be edited manually; environments decide whether to apply those catalog/dependency updates automatically or manually:

| Environment Auto-Sync | Behavior |
|---|---|
| ✅ **ON** | **Fully automatic**: Catalog auto updates (or manual edits) trigger immediate `Sync From Catalog`; dependency events trigger immediate `Sync With Cluster`. |
| ❌ **OFF** | **Manual environment sync**: Catalog auto updates/manual edits produce `Sync From Catalog` prompts; dependency events produce `Sync With Cluster` prompts. |

**Use Cases:**

- **Automatic Environments (ON)**: Fast-moving dev or QA namespaces that must stay aligned with production without human intervention.
- **Manual Environments (OFF)**: Developer sandboxes or branch environments where teams choose when to integrate upstream changes.

Default posture is **Manual (OFF)** for new environments to minimize surprise changes; users must explicitly opt into environment-level automation. Auto-mode executes both actions as needed.

## Architecture

### Components

```
┌─────────────────────────────────────────┐
│  Kubernetes Cluster (Source)            │
│  ┌───────────────────────────────────┐  │
│  │  Production Namespace             │  │
│  │  - Deployments                    │  │
│  │  - StatefulSets                   │  │
│  │  - DaemonSets                     │  │
│  │  - ReplicaSets, Jobs, CronJobs    │  │
│  │  - (ConfigMaps/Secrets/Services)  │  │
│  │    ↑ Not watched by catalog       │  │
│  └───────────────────────────────────┘  │
│           │                              │
│           │ Watch API (workloads only)   │
│           ▼                              │
│  ┌───────────────────────────────────┐  │
│  │  lapdev-kube-manager              │  │
│  │  - Watches ALL resources in       │  │
│  │    configured namespaces          │  │
│  │  - Forwards raw events            │  │
│  │  - No filtering logic             │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
           │
           │ RPC: report_resource_change()
           │ (namespace, type, name, yaml)
           ▼
┌─────────────────────────────────────────┐
│  Lapdev API Server                      │
│  ┌───────────────────────────────────┐  │
│  │  Change Event Processor           │  │
│  │  - Receives raw change events     │  │
│  │  - Maps to catalogs/environments  │  │
│  │  - Filters relevant changes       │  │
│  └───────────────────────────────────┘  │
│           │                              │
│           ▼                              │
│  ┌───────────────────────────────────┐  │
│  │  Sync Decision Engine             │  │
│  │  - Workload change? → catalog     │  │
│  │  - Config/Service? → environments │  │
│  │  - Check auto_sync flags          │  │
│  │  - Create sync records            │  │
│  └───────────────────────────────────┘  │
│           │                              │
│           ▼                              │
│  ┌───────────────────────────────────┐  │
│  │  Catalog Sync Logger              │  │
│  │  - Creates sync status record     │  │
│  │  - Notifies dashboard             │  │
│  │  - Emits webhooks/metrics         │  │
│  └───────────────────────────────────┘  │
│           │                              │
│           │ Auto-applies workload spec   │
│           ▼                              │
│  ┌───────────────────────────────────┐  │
│  │  Catalog Updater                  │  │
│  │  - Updates kube_app_catalog       │  │
│  │  - Updates workload specs         │  │
│  └───────────────────────────────────┘  │
│           │                              │
│           │ Trigger environment sync     │
│           ▼                              │
│  ┌───────────────────────────────────┐  │
│  │  Environment Sync Orchestrator    │  │
│  │  - Filters auto_sync envs         │  │
│  │  - Queues manual envs             │  │
│  │  - Calls kube-manager to apply    │  │
│  │  - Updates dependency index       │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
           │
           │ RPC: update_environment_workloads()
           ▼
┌─────────────────────────────────────────┐
│  Kubernetes Cluster (Target)            │
│  ┌───────────────────────────────────┐  │
│  │  Dev Environment Namespaces       │  │
│  │  - Updates workload specs         │  │
│  │  - Discovers ConfigMaps from prod │  │
│  │  - Discovers Secrets from prod    │  │
│  │  - Discovers Services from prod   │  │
│  │  - Applies changes                │  │
│  └───────────────────────────────────┘  │
└─────────────────────────────────────────┘
```

## End-to-End Flow

### A. Production Change → Lapdev API
1. Production resource changes (Deployment/StatefulSet/etc.) surface through the Kubernetes Watch stream handled by kube-manager.
2. Kube-manager forwards the raw event to the Lapdev API server via `report_resource_change`.
3. Change Event Processor normalizes/merges events for the same workload within the dedupe window.
4. Sync Decision Engine maps the event to affected catalogs (for workload ownership) and environments (via dependency index) without mutating catalog data.
5. Impacted environments receive notification records directly; catalogs only receive activity entries for visibility.

### B. Catalog Ownership Change → Environment Sync
1. User edits the App Catalog (add/remove workload). Catalog Updater writes the new spec into `kube_app_catalog_workload` and stamps `last_synced_at`.
2. Environment Sync Orchestrator enumerates impacted environments and splits them into auto-sync and manual buckets.
3. Auto-sync environments queue execution jobs that call kube-manager to reconcile workloads and re-discover dependent ConfigMaps/Secrets/Services.
4. Manual environments receive notifications referencing the originating `kube_app_catalog_sync_status`; when a user clicks "Sync From Catalog", the orchestrator enqueues a reconciliation job identical to the auto-sync path.
5. Reconciliation outcome is persisted back on the environment (`last_catalog_synced_at` and/or `last_dependency_synced_at`, plus error metadata if failures occur) and surfaced in UI/alerts; `latest_catalog_sync_id` is advanced to the newly applied record.
6. Dependency index updates record which source ConfigMaps/Secrets each environment now references, ensuring future events fan out correctly.

### C. Production Dependency Change → Environment Prompt
1. ConfigMap/Secret event arrives; Change Event Processor looks up impacted environments via `kube_environment_dependency`.
2. For each environment, create or update a `kube_environment_dependency_sync_status` record summarizing resource names, change types, and detection time, and set `latest_dependency_sync_id` to point at it.
3. Auto-sync environments enqueue `Sync With Cluster` jobs immediately; manual environments receive notifications with counts per resource type.
4. When sync completes, the dependency sync status transitions to `completed` (or `failed` with error payload), `last_dependency_synced_at` is updated, and `latest_dependency_sync_id` is cleared or replaced with the next pending record.

### Sync Record State Machine

| State | Trigger | Next States | Notes |
|---|---|---|---|
| `auto_applied` | Cluster change detected and catalog auto-updated | `failed`, _terminal_ | Catalog update applies immediately; `actor_id = NULL`; environment sync queued |
| `applied` | User saves catalog edit | `failed`, _terminal_ | Catalog update applies immediately; `actor_id` records user; environment sync queued |
| `failed` | Catalog edit apply fails | `retrying`, `manual_intervention` | Failure reason persisted for diagnosis |
| `retrying` | Automatic retry scheduled/executing | `applied`, `failed` | Retry budget/interval configurable |
| `dismissed` | User dismisses catalog notification without editing | _terminal_ | Keeps audit trail without spec change |
| `manual_intervention` | Automated retries exhausted or blocked | _terminal_ | Signals operators to intervene |

State machine extensions (`retrying`, `manual_intervention`) go beyond the minimal DDL but capture expected lifecycle so future iterations can expand persistence as needed.

`kube_environment_dependency_sync_status` follows a similar lifecycle:

| State | Trigger | Next States | Notes |
|---|---|---|---|
| `pending` | ConfigMap/Secret event detected, manual environment | `in_progress`, `dismissed` | Surfaces counts for `Sync With Cluster` CTA |
| `in_progress` | Sync job (auto or manual) executing | `completed`, `failed` | Job references this id for audit/retries |
| `completed` | Dependency refresh succeeded | _terminal_ | Updates `last_dependency_synced_at` |
| `failed` | Dependency refresh failed | `retrying`, `manual_intervention` | Stores error payload for UI |
| `retrying` | Automatic retry scheduled/executing | `completed`, `failed` | Retry budget shared with catalog syncs |
| `dismissed` | User dismisses notification without syncing | _terminal_ | Maintains audit trail without applying changes |
| `manual_intervention` | Automatic retries exhausted | _terminal_ | Signals operators to inspect cluster-side issues |

### Database Changes

#### New Table: `kube_app_catalog_sync_status`

```sql
CREATE TABLE kube_app_catalog_sync_status (
    id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL,
    app_catalog_id UUID NOT NULL REFERENCES kube_app_catalog(id),
    status VARCHAR NOT NULL, -- 'auto_applied','applied','failed','retrying','dismissed','manual_intervention'
    detected_at TIMESTAMPTZ NOT NULL,
    actor_id UUID REFERENCES users(id), -- null for system-generated notifications
    workload_count INTEGER NOT NULL, -- Simple count: how many workloads changed
    workload_names TEXT[] NOT NULL -- Array of workload names that changed
);
```

**Storage Strategy:**
- **Simple summary only**: Just count + list of workload names
- `status = auto_applied` captures production-driven updates; `status = applied` records manual catalog edits
- `actor_id` stores the user that applied a catalog edit; system-driven auto updates leave it `NULL`
- No detailed diffs stored (not needed for immediate application)
- Environments reference this record via `latest_catalog_sync_id` to know when a catalog change is available to sync

#### New Table: `kube_environment_configmap`

```sql
CREATE TABLE kube_environment_configmap (
    id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ,
    environment_id UUID NOT NULL REFERENCES kube_environment(id),
    name VARCHAR NOT NULL,
    namespace VARCHAR NOT NULL,
    data JSONB NOT NULL,
    UNIQUE(environment_id, namespace, name)
);
```

#### New Table: `kube_environment_secret`

```sql
CREATE TABLE kube_environment_secret (
    id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL,
    deleted_at TIMESTAMPTZ,
    environment_id UUID NOT NULL REFERENCES kube_environment(id),
    name VARCHAR NOT NULL,
    namespace VARCHAR NOT NULL,
    data JSONB NOT NULL, -- Encrypted
    type VARCHAR NOT NULL, -- Opaque, kubernetes.io/tls, etc.
    UNIQUE(environment_id, namespace, name)
);
```

#### New Table: `kube_environment_dependency`

```sql
CREATE TABLE kube_environment_dependency (
    id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL,
    environment_id UUID NOT NULL REFERENCES kube_environment(id),
    resource_type VARCHAR NOT NULL, -- 'configmap' or 'secret'
    source_namespace VARCHAR NOT NULL,
    source_name VARCHAR NOT NULL,
    target_namespace VARCHAR NOT NULL,
    target_name VARCHAR NOT NULL,
    last_discovered_at TIMESTAMPTZ NOT NULL,
    CHECK (resource_type IN ('configmap','secret')),
    UNIQUE(environment_id, resource_type, source_namespace, source_name)
);
```

#### New Table: `kube_environment_dependency_sync_status`

```sql
CREATE TABLE kube_environment_dependency_sync_status (
    id UUID PRIMARY KEY,
    created_at TIMESTAMPTZ NOT NULL,
    environment_id UUID NOT NULL REFERENCES kube_environment(id),
    status VARCHAR NOT NULL, -- 'pending','in_progress','completed','failed','retrying','dismissed','manual_intervention'
    detected_at TIMESTAMPTZ NOT NULL,
    resolved_at TIMESTAMPTZ,
    resolved_by UUID REFERENCES users(id),
    resource_summaries JSONB NOT NULL, -- Array of {resource_type, namespace, name, change_type}
    auto_triggered BOOLEAN NOT NULL DEFAULT false
);
```

#### Modified Table: `kube_environment`

```sql
ALTER TABLE kube_environment
ADD COLUMN auto_sync BOOLEAN NOT NULL DEFAULT false;

ALTER TABLE kube_environment
ADD COLUMN last_catalog_synced_at TIMESTAMPTZ;

ALTER TABLE kube_environment
ADD COLUMN last_dependency_synced_at TIMESTAMPTZ;

ALTER TABLE kube_environment
ADD COLUMN latest_catalog_sync_id UUID REFERENCES kube_app_catalog_sync_status(id);

ALTER TABLE kube_environment
ADD COLUMN latest_dependency_sync_id UUID REFERENCES kube_environment_dependency_sync_status(id);
```

**Note:** When catalog updates, we store references to the catalog sync (`latest_catalog_sync_id`) and dependency sync (`latest_dependency_sync_id`) records that triggered notifications. The environment doesn't need its own detailed diff - users can click through to these records to see details. The dependency index (`kube_environment_dependency`) enables constant-time lookup when a production ConfigMap/Secret event arrives, while `kube_environment_dependency_sync_status` tracks pending dependency changes that power the `Sync With Cluster` UI.

#### Modified Table: `kube_app_catalog`

```sql
ALTER TABLE kube_app_catalog
ADD COLUMN last_synced_at TIMESTAMPTZ;

ALTER TABLE kube_app_catalog
ADD COLUMN source_namespace VARCHAR NOT NULL; -- Which namespace to watch in the source cluster
```

**Note:**
- `source_namespace` tells kube-manager which namespace to watch

**Namespace Watch Configuration:**
- API server aggregates all `source_namespace` values for a given cluster
- Sends list of namespaces to kube-manager: `["production", "staging"]`
- Kube-manager watches ALL resources in those namespaces
- API server filters events to determine which belong to which catalog

## Implementation Plan

### Phase 1: Watch Infrastructure (Kube-Manager)

**Kube-Manager Changes (Deployed in customer clusters):**
1. Implement namespace configuration RPC endpoint:
   - `configure_watches(namespaces: Vec<String>)`
   - Kube-manager stores namespace list
   - Dynamically start/stop watches when namespace list changes
2. Add Kubernetes Watch for ALL resources in configured namespaces:
   - Deployments, StatefulSets, DaemonSets, ReplicaSets, Jobs, CronJobs
   - ConfigMaps
   - Secrets
   - Services
3. Implement event forwarding (no filtering):
   - Capture: namespace, resource_type, resource_name, change_type (created/updated/deleted), full YAML
   - Forward ALL events to API server via RPC
4. Add RPC method: `report_resource_change(event: ResourceChangeEvent)`

**API Server Changes (Centralized service):**
1. Implement namespace watch configuration management:
   - When catalog created/updated: aggregate `source_namespace` per cluster
   - Send `configure_watches([namespaces])` RPC to kube-manager
   - Track which namespaces are being watched per cluster
2. Implement `report_resource_change` RPC handler
3. Build **Change Event Processor**:
   - Receive raw events from kube-manager
   - Map workload events to catalogs:
     - Query: Which catalog has workload X in namespace Y?
   - Map ConfigMap/Secret/Service events to environments:
     - Query: Which environments reference ConfigMap X in namespace Y?
4. Build **Sync Decision Engine**:
   - Workload events → Check if belongs to any catalog → Record catalog activity entry (`status = auto_applied`, actor_id = NULL)
   - ConfigMap/Secret/Service events → Check if belongs to any environment → Trigger environment sync
   - Check environment `auto_sync` flags to decide immediate reconcile vs manual prompt
   - Use dependency index (`kube_environment_dependency`) to map source resources to environments and create/update `kube_environment_dependency_sync_status`
5. Implement event deduplication (ignore duplicate events within time window)

### Phase 2: Sync Status & Notifications
1. Create `kube_app_catalog_sync_status` table.
2. Implement sync status API endpoints for querying recent catalog updates.
3. Record catalog activity entries on production changes (`status = auto_applied`) and catalog edits (`status = applied`).
4. Publish dashboard and activity feed surfaces for production detections and user-applied catalog edits.
5. Emit webhook/notification events (if enabled) to inform downstream systems of catalog activity (without auto-mutating specs).
6. Provide API utilities to resolve ConfigMap/Secret events to environments via `kube_environment_dependency` and expose `kube_environment_dependency_sync_status` summaries (updating `latest_dependency_sync_id`).

### Phase 3: Catalog Update
1. Implement catalog update logic invoked when users save catalog edits
2. Update `kube_app_catalog_workload` specs based on the saved user edits (optionally sourcing manifests from production for new workloads)
3. Update `kube_app_catalog.last_synced_at` timestamp
4. Trigger environment sync flow (Phase 4)
5. Track version history (optional)

### Phase 4: Environment Sync
1. Add `auto_sync` flag to `kube_environment`
2. Create `kube_environment_configmap` and `kube_environment_secret` tables (if not already exist)
3. Create `kube_environment_dependency` and `kube_environment_dependency_sync_status` tables
4. Implement environment sync orchestrator
5. Add RPC method: `sync_environment(env_id, sync_request)`
   - For `sync_request.action = 'catalog'`: updates workload specs from catalog, then cascades discovery for dependencies/services
   - For `sync_request.action = 'dependency'`: refreshes ConfigMaps/Secrets/Services only, based on dependency index
   - Payload includes `catalog_sync_id` or `dependency_sync_id` to support auditing and idempotency
6. Handle both shared and branch environments
7. Implement ConfigMap/Secret discovery and update logic with proper encryption for Secrets
8. Persist dependency mappings in `kube_environment_dependency` for each source ConfigMap/Secret
9. Implement service discovery and preview URL updates
10. Update `kube_environment.last_catalog_synced_at` and/or `last_dependency_synced_at` timestamps based on action completed
11. Audit manual triggers by storing initiating user id, catalog sync id, dependency event ids, and outcome for every `manual_trigger=true` job

### Phase 5: Dashboard & Notifications
1. Add notification system for catalog activity and environment sync prompts.
2. Create **catalog activity UI** summarizing applied changes (count + workload names).
3. Create **environment notification UI** for catalog updates ("Sync From Catalog") and dependency updates ("Sync With Cluster") plus post-sync results.
4. Add environment auto-sync toggle in environment settings.
5. Add environment sync status indicators.
6. Add manual sync triggers for environments with auto-sync disabled.
7. Implement notification badges:
   - Catalog: "Production change detected - 3 workloads" (notification) vs "Edit applied - workload foo added"
   - Environment: "Catalog update available - 3 workloads" vs "Config update available - 2 ConfigMaps" (manual) and a single "Syncing..." state for auto actions
8. Support dismiss actions that update dependency sync status (set to `dismissed`) or log catalog deferrals without applying changes.

## Detailed Implementation Steps

1. **Schema & Persistence**
   - [ ] Draft migration plan
     - [ ] Document new tables/columns, data types, indexes, FK relationships.
     - [ ] Review migration design with Data Platform (sizing, retention, encryption).
   - [ ] Implement migrations
     - [ ] Create tables `kube_environment_configmap`, `kube_environment_secret`, `kube_environment_dependency`, `kube_environment_dependency_sync_status`.
     - [ ] Alter `kube_app_catalog_sync_status` and `kube_environment` to add new statuses/timestamps/references.
     - [ ] Add supporting indexes (config/secret uniqueness, dependency lookup, catalog status filtering).
     - [ ] Add constraints/foreign keys with documented ON DELETE behavior.
   - [ ] Backfill legacy data
     - [ ] Snapshot current environment resources (scripts/queries).
     - [ ] Populate config/secret tables (respect encryption requirements).
     - [ ] Build & run dependency discovery seeding script.
     - [ ] Seed catalog activity rows with `status = auto_applied`, `actor_id = NULL` (representing current production state).
   - [ ] Validate & harden
     - [ ] Add automated tests verifying row counts/integrity post-backfill.
     - [ ] Prepare rollback SQL for each migration.
     - [ ] Write migration runbook (order, downtime, verification).

2. **Kube-Manager Enhancements**
   - [ ] Implement `configure_watches` RPC (handler, diffing, persistence, retries).
   - [ ] Extend resource watchers to include workloads/configmaps/secrets/services with full YAML payloads.
   - [ ] Add event queuing/backpressure controls and Prometheus metrics (latency, queue depth, resource version lag).
   - [ ] Write integration tests (namespace changes, reconnects, event storms).
   - [ ] Update deployment artifacts (Helm/manifests), permissions, and release notes.

3. **Lapdev API – Event Processing**
   - [ ] Build Change Event Processor endpoint (validation, YAML parsing, auth).
   - [ ] Implement workload event flow (catalog ownership lookup, dedupe, create `auto_applied` activity with `actor_id = NULL`, enqueue notification job).
   - [ ] Implement dependency event flow (dependency lookup, upsert sync status, auto-sync queueing, update environment `latest_dependency_sync_id`).
   - [ ] Add dedupe windowing + stale dependency guard rails (rebuild triggers).
   - [ ] Expose APIs to list catalog activity and dependency statuses (with tests).

4. **Catalog Management**
   - [ ] Update backend edit handlers (apply workload adds/removes, optional manifest fetch).
   - [ ] Insert `status = applied` activity rows with actor metadata and trigger environment sync orchestration.
   - [ ] Build catalog activity listing endpoints + UI components (feed, details, filters).
   - [ ] Implement catalog dismiss audit endpoint and UI action.
   - [ ] Add automated/unit/UI tests for edits, rollbacks, activity log correctness.

5. **Environment Sync Orchestrator**
   - [ ] Define `sync_environment` RPC schema and job queue wiring.
   - [ ] Implement catalog sync worker (workload apply, dependency refresh, service updates, update environment timestamps, set `latest_catalog_sync_id` to applied record).
   - [ ] Implement dependency sync worker (resource fetch, table updates, dependency pruning, update status/timestamps, clear or advance `latest_dependency_sync_id`).
   - [ ] Add concurrency controls (per-environment locks) and execution telemetry logging.
   - [ ] Implement retry/backoff policy and error reporting surfaced to API/UI.

6. **User Experience & Notifications**
   - [ ] Build catalog activity feed UI (notification cards, workload links, dismiss action).
   - [ ] Build environment sync UI (pending actions panel, `Sync From Catalog` / `Sync With Cluster` modals, history).
   - [ ] Add environment settings page for auto-sync toggles and reminders.
   - [ ] Extend notification service for Slack/email/webhooks with consistent payloads.
   - [ ] Instrument UX analytics (CTA usage, dismissals, sync outcomes) and dashboard.

7. **Observability & Operations**
   - [ ] Instrument metrics (event ingest, dedupe, queue depth, sync latency, failure/dismiss counts) with tagging.
   - [ ] Define alerts and write runbooks (watch health, backlog, failure spikes, stale prompts, dismissal spikes).
   - [ ] Implement feature flags/kill switches (environment auto-sync, catalog notifications, dependency auto-sync) plus documentation.
   - [ ] Deliver operational tooling (CLI/API for manual sync, dependency rebuild, dismissal reset) and train support/SRE teams.

8. **Rollout & Validation**
   - [ ] Internal dogfood: enable flags, simulate production changes, test dismiss/sync flows, run chaos scenarios.
   - [ ] Design partner preview: enable for selected customers, collect qualitative/quantitative feedback, iterate on thresholds/UX.
   - [ ] Progressive auto-sync: allow opt-in, instrument success/failure, shadow dependency auto-sync before enabling writes.
   - [ ] General availability: publish docs/support materials, default new environments to manual, monitor telemetry/support closely, plan post-release review.
## Workstream Overview & Dependencies

| Workstream | Scope | Lead Team | Key Dependencies |
|---|---|---|---|
| Watch Infrastructure | Kube-manager watch plumbing, RPC schema, API ingestion | Platform Agents | Requires namespace metadata on catalogs |
| Sync Decision Engine | Event classification, dedupe, sync record lifecycle | Backend Services | Needs Watch Infrastructure |
| Catalog & Environment Persistence | DDL migrations, ORM updates, encryption plumbing | Data Platform | Sequenced before UI/API consumption |
| Orchestration & Queues | Catalog update executor, environment reconciliation workers | Backend Services | Depends on Persistence schema |
| UI & Notification Layer | Dashboard surfaces, toggles, notification delivery | Frontend & UX | Needs API endpoints & sync records |
| Observability & Ops | Metrics, alerting, runbooks, failure injection | SRE | Consumes telemetry hooks from all layers |

Critical path: Watch Infrastructure → Sync Decision Engine → Persistence → Orchestration. UI/Notification and Observability can parallelize once APIs are stable.

## Rollout Strategy

1. **Internal Dogfood (Phase 1–3)**  
   - Target a non-production catalog within Lapdev infrastructure.  
   - Validate production-change notifications with environments in manual mode; ensure catalog activity logs populate without mutating specs.
2. **Design Partner Preview (Phase 4)**  
   - Select 1–2 customers with tolerant workloads.  
   - Gate behind feature flag: catalog notifications enabled, environments default manual.  
   - Collect SLA metrics (event latency, sync duration) and monitor false-positive rate.
3. **Progressive Environment Auto-Sync Enablement (Phase 5)**  
   - Offer environment auto-sync opt-in to preview customers once reconciliation stability is proven.  
   - Shadow auto-sync (dry-run) for high-sensitivity environments before enabling writes.
4. **General Availability**  
   - Document operational runbooks.  
   - Enable environment auto-sync opt-in for all customers, default OFF.  
   - Announce with clear safety guidelines and fallback steps.

Each stage requires explicit go/no-go review covering error budget impact, support readiness, and security sign-off.

## Edge Cases

### Workload Deleted in Production
- Mark workload as "missing" in catalog
- Show warning in dashboard
- Don't auto-remove from catalog (user decides)

### ConfigMap/Secret/Service Discovery Failures
- If environment sync can't discover ConfigMaps/Secrets/Services from production
- Show error: "Failed to sync ConfigMap 'app-config' from production - does not exist"
- Environment keeps using existing resources until production is fixed
- User can manually intervene or wait for production to be restored

### Conflicting Changes in Branch Environment
- If branched workload was manually modified AND catalog updated
- Show conflict notification
- User chooses: keep branch changes or sync from catalog

### Dependency Events During Pending Catalog Sync
- ConfigMap/Secret change may fire before environment adopts latest catalog
- Environment shows both prompts; users can run `Sync With Cluster` to unblock secrets while deferring workload adoption
- Auto-sync environments execute catalog and dependency tasks sequentially (catalog first, then dependency)

### Watch Connection Loss
- Detect websocket/watch disconnection
- Reconnect automatically
- Resync state after reconnection (compare current vs last known state)

### Multiple Catalog Changes in Quick Succession
- Batch changes into single sync request (within 30 second window)
- Show combined notification: "5 workloads changed"
- List affected workload names

### New Catalog Created or Namespace Changed
- When catalog is created or `source_namespace` is updated
- API server recalculates namespace watch list for the cluster
- Sends `configure_watches([new_namespace_list])` to kube-manager
- Kube-manager dynamically adds/removes watches as needed
- No restart required

### Catalog Deleted
- API server recalculates namespace watch list
- If namespace no longer has any catalogs, stop watching it
- Send updated `configure_watches()` to kube-manager
- Reduces unnecessary event traffic

### Kube-Manager Receives Unknown Events
- Kube-manager forwards ALL events (even if API server doesn't recognize them)
- API server silently ignores events that don't map to any catalog/environment
- No error logged (normal behavior - cluster may have resources we don't track)

### Event Storm (Many Rapid Changes)
- Kube-manager forwards all events (no throttling at kube-manager level)
- API server implements deduplication:
  - Batch events within 30-second window
  - Group by resource (namespace + type + name)
  - Only process latest version of resource
- Prevents creating multiple sync records for same resource

## Future Enhancements

1. **Selective Field Sync**: User chooses which fields to sync (e.g., always sync images, but not resource limits)
2. **Rollback**: Revert catalog to previous version if sync causes issues
3. **Dry Run**: Preview what would change in environments before applying
4. **Scheduled Sync**: Define maintenance windows for environment auto-sync
5. **Notifications**: Slack/email notifications for environment sync prompts

## Security Considerations

1. **RBAC**: Only catalog editors can modify watch settings; environment sync actions require environment write access.
2. **Audit Log**: Record all catalog edits (who changed workloads) and manual environment sync invocations with actor (or system) and timestamp.
3. **Validation**: Validate workload specs before applying (resource limits, security contexts)
4. **Rate Limiting**: Prevent sync storms (max N syncs per minute)
5. **Secret Handling**:
   - Secrets stored encrypted in `kube_environment_secret` table
   - Diff view shows key names and change indicators, not actual secret values
   - Secret values only transmitted over TLS between components
   - RBAC: Separate permission for viewing Secret diffs
   - Audit: Log all Secret access and modifications

## Testing Strategy

1. **Unit Tests**:
   - Kube-manager: Event forwarding, namespace watch configuration
   - API server: Event mapping, sync decision logic, deduplication
   - Catalog/environment sync logic, dependency fan-out resolution

2. **Integration Tests**:
   - End-to-end flow: Production change → Kube-manager event → API decision → Catalog/environment sync
   - Namespace watch reconfiguration when catalogs added/removed
   - Auto-sync vs manual environment workflows
   - ConfigMap/Secret/Service discovery during environment sync with dependency index updates

3. **Load Tests**:
   - 100+ workloads across multiple namespaces
   - Rapid changes (event storm simulation)
   - Event deduplication effectiveness
   - Multiple catalogs watching same namespace

4. **Failure Tests**:
   - Watch disconnection and reconnection
   - Network failures between kube-manager and API server
   - Partial updates (only some workloads sync successfully)
   - Kube-manager restart (watches re-established)
   - API server restart (watch configuration re-sent)

5. **Edge Case Tests**:
   - Unknown events (resources not in any catalog)
   - Namespace watch list changes during active sync
   - Catalog deleted shortly after edit applied
   - Multiple overlapping syncs for same catalog

## Metrics & Monitoring

**Kube-Manager Metrics:**
- Events forwarded per second (by resource type)
- Watch connection health per namespace
- RPC failures to API server
- Event queue depth (if batching)

**API Server Metrics:**
- Events received per second (total and by cluster)
- Events mapped to catalogs/environments (hit rate)
- Events ignored (not mapped to any resource)
- Sync detection latency (time from event to sync decision)
- Deduplication effectiveness (events deduplicated / total events)

**Sync Workflow Metrics:**
- Catalog edit latency (user save → catalog applied)
- Environment sync latency (catalog updated → environment synced)
- Environment decision latency (catalog applied → user-triggered sync) for manual environments
- Dependency sync latency (event detected → dependency sync completed)
- Sync success/failure rates
- Number of environments awaiting manual sync
- Auto-sync vs manual-sync ratio split by action type (catalog vs dependency)
- Dismissal rate for dependency prompts (count of statuses marked `dismissed`)

**Resource Metrics:**
- Number of namespaces watched per cluster
- Number of resources watched per namespace
- Active watches per cluster
- Dependency index size per cluster (ConfigMap/Secret links)

## Operational Readiness
- **Runbooks:** Document common failure paths (watch disconnect, sync failure, secret decryption error, dependency sync backlog) including diagnostic commands and escalation contacts.
- **Alerting:** Trigger alerts when sync latency or failure rate breach thresholds, or when no events are received for a watched namespace within N minutes.
- **Backfill:** Provide a CLI/API command to backfill sync records after outages; ensures state catch-up without manual DB edits.
- **Feature Flags:** Maintain kill switches for environment auto-sync (and catalog notifications) to halt rollout quickly if issues emerge.
- **Access Controls:** Verify permissions for support engineers to inspect sync status without granting ability to trigger environment syncs.

## Risks & Mitigations
- **False Positives/Noise:** Deduplication window and workload ownership mapping errors could generate noisy syncs. Mitigation: add resource ownership cache, alert on high manual deferral rates.
- **Secrets Exposure:** Transporting full secret data increases blast radius. Mitigation: enforce envelope encryption at rest, limit log redaction, and gate diff visibility behind elevated RBAC.
- **Customer Cluster Load:** Watching all resources could stress API servers. Mitigation: allow namespace-level sampling configuration and backoff when resource version drift detected.
- **Long-Running Branch Mods:** Auto-sync might overwrite intentional branch divergences. Mitigation: branch environments default to manual; surface conflicts with ability to reapply branch overrides.
- **Partial Failures:** Catalog update succeeding while environment sync fails leaves inconsistent state. Mitigation: persist failure reason, expose retry control, and guard rails to prevent infinite retries.
- **Stale Dependency Index:** Missed cleanup could cause redundant environment sync prompts. Mitigation: rebuild dependencies on each environment sync and schedule periodic reconciliation jobs.
- **Excessive Dismissals:** Users may repeatedly dismiss dependency prompts, leaving environments outdated. Mitigation: surface dismissal metrics, add reminder nudges, and allow policy to cap consecutive dismissals.

## Open Questions

1. Should we allow users to bundle "Sync From Catalog" and "Sync With Cluster" into a single action?
   - Recommendation: Keep CTAs separate in v1 for clarity; evaluate combined apply after observing user behavior.
2. Should we support partial environment sync (only sync specific workloads)?
   - Recommendation: Defer to future iteration; scope for v1 is full-environment reconciliation with branch overrides preserved.
3. How long should we retain catalog sync activity and failure records?
   - Recommendation: Retain 90 days online, archive thereafter for compliance reviews.
4. Should branch environments optionally "pin" to a catalog version?
   - Recommendation: Provide catalog pinning as a branch-level setting post-v1; for now rely on manual sync deferral.
5. Do we need sync notifications via webhook (for CI/CD integration)?
   - Recommendation: Capture requirement from design partners; build webhook emitter after GA if needed.
6. Should there be a "pause sync" option to temporarily disable watches without changing environment auto-sync settings?
   - Recommendation: Yes, add a `pause_until` field so teams can suspend watches during planned maintenance.
7. Do we need a catalog "dry run" mode that records detections without applying to workloads?
   - Recommendation: Consider a shadow-only mode that writes sync records with `preview_only=true` while skipping updates, primarily for analytics prior to rollout adjustments.
8. Should the dependency index be extended to Services or other resource types?
   - Recommendation: Track ConfigMaps/Secrets in v1; evaluate Service inclusion after we validate performance of the new index.

**Architecture Questions:**
1. Should kube-manager batch events before sending to API server, or send immediately?
   - Decision: Send immediately for low latency; API server handles batching/deduplication.
2. How should we handle very large resources (e.g., ConfigMap with 10MB of data)?
   - Recommendation: Send checksum + metadata via event; API server fetches full resource on-demand when diffing.
3. Should we implement event replay/audit log?
    - Recommendation: Store raw events for 7 days in object storage with index for compliance, gated behind feature flag.
4. What happens if kube-manager falls behind (event backlog)?
    - Recommendation: Queue with bounded buffer, emit alert at 70% capacity, allow API server to request resync.
5. Should watch configuration be persisted in kube-manager?
    - Decision: Fetch from API server on startup; minimal local persistence required beyond last successful revision.
