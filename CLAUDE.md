# Lapdev Kubernetes Integration Project

## Current State
Working on **kube** branch implementing Kubernetes provider functionality for Lapdev (self-hosted remote dev environment platform).

## Architecture Overview
- **lapdev-kube**: Core Kubernetes integration library with provider abstractions
- **lapdev-kube-rpc**: RPC definitions for Kubernetes operations
- **lapdev-kube-manager**: Service binary that manages Kubernetes clusters
- **lapdev-hrpc**: Custom HTTP-based RPC framework
- **lapdev-hrpc-proc-macro**: Procedural macros for HRPC

## Key Components Implemented

### Database Schema
- `k8s_provider`: Stores Kubernetes provider configurations (GCP initially)
- `kube_cluster`: Cluster information and connection details
- `kube_cluster_token`: Authentication tokens for cluster access

### Service Architecture
- `lapdev-kube-manager` binary for cluster management
- HRPC-based communication between services
- Token-based authentication system with `KUBE_CLUSTER_TOKEN_HEADER`

## Current Implementation Status

### ✅ Completed
- Workspace dependencies and project structure
- Basic Kubernetes types and provider enums
- Database entities for K8s providers and clusters
- GCP provider scaffold
- HRPC framework foundation

### 🚧 In Progress
- Kubernetes cluster connection logic
- Provider-specific implementations
- Manager service functionality
- Currently working on implement lapdev-kube-manager, and the first step is to report cluster info to KubeClusterServer via KubeClusterRpc

### 📋 TODO
- Implement cluster lifecycle management
- Add workspace scheduling to Kubernetes
- Integration with existing lapdev-conductor
- Testing and validation

## Development Commands
```bash
# Build entire workspace
cargo build

# Run specific services
cargo run --bin lapdev-kube-manager

# Database migrations
./lapdev-db/run_migration.sh

# Generate database entities
./lapdev-db/generate_entities.sh
```

## Key Files to Focus On
- `lapdev-kube-manager/src/manager.rs` - Cluster management
- `lapdev-kube/src/server.rs` - Kubernetes API server
- `lapdev-common/src/kube.rs` - Shared Kubernetes types

## Notes
- Using tarpc for RPC but also implementing custom HRPC framework
- Kubernetes client library: `kube = "1.1.0"` with `k8s-openapi = "0.25.0"`
- Database: SeaORM with PostgreSQL

## Service Details
- lapdev-kube-manager is a service which runs inside k8s cluster itself

## Development Tips
- For checking compile error, only do cargo check instead of cargo build, because it's much faster

## AI Assistant Guidelines
ALWAYS consider whether any available sub-agents could help with the current task, even for seemingly simple requests. Before responding directly, evaluate:
- Which sub-agents are available (general-purpose, rust-postgres-backend-expert, rpc-backend-architect, leptos-frontend-expert, rust-k8s-backend-engineer)
- Whether any part of the task falls within their expertise
- If delegating would provide better results

Use sub-agents proactively, not just when explicitly asked. Available specialized agents:
- **rust-k8s-backend-engineer**: For Kubernetes integration, kube-rs, cluster management, operators, authentication, RBAC
- **rust-postgres-backend-expert**: For database schema, SeaORM, query optimization, migrations
- **rpc-backend-architect**: For RPC systems, tarpc, HRPC, service communication, distributed systems
- **general-purpose**: For complex multi-step tasks, code searches, research