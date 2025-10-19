use anyhow::{anyhow, Context as _, Result as AnyResult};
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{PodSpec, Service},
};
use lapdev_common::kube::{
    KubeClusterInfo, KubeContainerImage, KubeContainerInfo, KubeContainerPort, KubeServicePort,
    KubeWorkloadKind,
};
use lapdev_db::api::{CachedClusterService, DbApi};
use lapdev_db_entities::kube_app_catalog_workload::{self, Entity as CatalogWorkloadEntity};
use lapdev_kube_rpc::{
    KubeClusterRpc, KubeManagerRpcClient, ResourceChangeEvent, ResourceChangeType, ResourceType,
};
use sea_orm::prelude::Json;
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use std::collections::{BTreeMap, HashMap, HashSet};
use std::sync::Arc;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::tunnel::TunnelRegistry;

/// KubeClusterServer is the central server where
/// KubeManager and KubeCli connects to
#[derive(Clone)]
pub struct KubeClusterServer {
    cluster_id: Uuid,
    pub rpc_client: KubeManagerRpcClient,
    db: DbApi,
    kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
    tunnel_registry: Arc<TunnelRegistry>,
}

impl KubeClusterServer {
    pub fn new(
        cluster_id: Uuid,
        client: KubeManagerRpcClient,
        db: DbApi,
        kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
        tunnel_registry: Arc<TunnelRegistry>,
    ) -> Self {
        Self {
            cluster_id,
            rpc_client: client,
            db,
            kube_cluster_servers,
            tunnel_registry,
        }
    }

    pub fn cluster_id(&self) -> Uuid {
        self.cluster_id
    }

    pub async fn register(&self) {
        let mut servers = self.kube_cluster_servers.write().await;
        servers
            .entry(self.cluster_id)
            .or_insert_with(Vec::new)
            .push(self.clone());
        tracing::info!(
            "Registered KubeClusterServer for cluster {}",
            self.cluster_id
        );
    }

    pub async fn unregister(&self) {
        let mut servers = self.kube_cluster_servers.write().await;
        if let Some(cluster_servers) = servers.get_mut(&self.cluster_id) {
            // Remove servers with matching cluster_id (in case there are multiple connections)
            let initial_len = cluster_servers.len();
            cluster_servers.retain(|s| s.cluster_id != self.cluster_id || !std::ptr::eq(s, self));

            if cluster_servers.len() < initial_len {
                tracing::info!(
                    "Unregistered KubeClusterServer for cluster {}",
                    self.cluster_id
                );
            }

            // Remove empty entries
            if cluster_servers.is_empty() {
                servers.remove(&self.cluster_id);
            }
        }
    }
}

impl KubeClusterRpc for KubeClusterServer {
    async fn report_cluster_info(
        self,
        _context: ::tarpc::context::Context,
        cluster_info: KubeClusterInfo,
    ) -> Result<(), String> {
        tracing::info!(
            "Received cluster info for cluster {}: {:?}",
            self.cluster_id,
            cluster_info
        );

        // Verify cluster exists, error out if not found
        let _cluster = self
            .db
            .get_kube_cluster(self.cluster_id)
            .await
            .map_err(|e| format!("Database error: {}", e))?
            .ok_or_else(|| format!("Cluster {} not found", self.cluster_id))?;

        // Update cluster info in database
        let status_str = format!("{:?}", cluster_info.status);
        self.db
            .update_kube_cluster_info(
                self.cluster_id,
                Some(cluster_info.cluster_version),
                Some(status_str),
                cluster_info.region,
            )
            .await
            .map_err(|e| format!("Failed to update cluster info: {}", e))?;

        tracing::info!(
            "Successfully updated cluster {} info in database",
            self.cluster_id
        );

        Ok(())
    }

    async fn tunnel_heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        tracing::debug!("Received heartbeat from cluster {}", self.cluster_id);

        self.tunnel_registry.update_heartbeat(self.cluster_id).await
    }

    async fn report_tunnel_metrics(
        self,
        _context: ::tarpc::context::Context,
        active_connections: u32,
        bytes_transferred: u64,
        connection_count: u64,
        connection_errors: u64,
    ) -> Result<(), String> {
        tracing::debug!(
            "Received tunnel metrics from cluster {}: connections={}, bytes={}, errors={}",
            self.cluster_id,
            active_connections,
            bytes_transferred,
            connection_errors
        );

        self.tunnel_registry
            .update_metrics(
                self.cluster_id,
                active_connections,
                bytes_transferred,
                connection_count,
                connection_errors,
            )
            .await
    }

    async fn report_resource_change(
        self,
        _context: ::tarpc::context::Context,
        event: ResourceChangeEvent,
    ) -> Result<(), String> {
        tracing::debug!(
            cluster_id = %self.cluster_id,
            namespace = %event.namespace,
            resource_name = %event.resource_name,
            resource_type = ?event.resource_type,
            change_type = ?event.change_type,
            "Received resource change event"
        );

        let result = match event.resource_type {
            ResourceType::Service => self.handle_service_change(&event).await,
            _ => self.handle_workload_change(&event).await,
        };

        if let Err(err) = result {
            tracing::error!(
                cluster_id = %self.cluster_id,
                namespace = %event.namespace,
                resource_name = %event.resource_name,
                error = ?err,
                "Failed to process resource change event"
            );
            return Err(err.to_string());
        }

        Ok(())
    }
}

impl KubeClusterServer {
    async fn handle_workload_change(&self, event: &ResourceChangeEvent) -> AnyResult<()> {
        let Some(workload_kind) = workload_kind_for(event.resource_type) else {
            // Ignore non-workload resources for now.
            return Ok(());
        };

        if matches!(event.change_type, ResourceChangeType::Deleted) {
            tracing::debug!(
                namespace = %event.namespace,
                resource_name = %event.resource_name,
                "Skipping deleted workload event (not yet handled)"
            );
            return Ok(());
        }

        let yaml = event
            .resource_yaml
            .as_ref()
            .context("workload event missing resource YAML")?;

        let extracted =
            extract_workload_from_yaml(event.resource_type, yaml).with_context(|| {
                format!(
                    "failed to parse workload YAML for {}/{} ({:?})",
                    event.namespace, event.resource_name, event.resource_type
                )
            })?;
        let new_containers = extracted.containers;
        let workload_labels = extracted.labels;

        if new_containers.is_empty() {
            tracing::warn!(
                namespace = %event.namespace,
                resource_name = %event.resource_name,
                "Parsed workload contains no containers; skipping update"
            );
            return Ok(());
        }

        let cached_services = self
            .db
            .get_active_cluster_services(self.cluster_id, &event.namespace)
            .await?;

        let workloads = CatalogWorkloadEntity::find()
            .filter(kube_app_catalog_workload::Column::Name.eq(event.resource_name.clone()))
            .filter(kube_app_catalog_workload::Column::Namespace.eq(event.namespace.clone()))
            .filter(kube_app_catalog_workload::Column::DeletedAt.is_null())
            .filter(kube_app_catalog_workload::Column::ClusterId.eq(self.cluster_id))
            .all(&self.db.conn)
            .await
            .with_context(|| {
                format!(
                    "failed querying catalog workloads for {}/{}",
                    event.namespace, event.resource_name
                )
            })?;

        if workloads.is_empty() {
            tracing::trace!(
                namespace = %event.namespace,
                resource_name = %event.resource_name,
                "Workload not tracked in any catalog; ignoring"
            );
            return Ok(());
        }

        for workload in workloads {
            // Ensure the stored kind matches; if not, skip but log.
            if let Ok(stored_kind) = workload.kind.parse::<KubeWorkloadKind>() {
                if stored_kind != workload_kind {
                    tracing::warn!(
                        namespace = %event.namespace,
                        resource_name = %event.resource_name,
                        stored_kind = %workload.kind,
                        event_kind = ?workload_kind,
                        "Catalog workload kind mismatch; skipping update"
                    );
                    continue;
                }
            }

            let merged_containers = merge_containers(&workload.containers, &new_containers)
                .with_context(|| {
                    format!(
                        "failed to merge container definitions for workload {}",
                        workload.id
                    )
                })?;
            let service_ports = ports_from_cached_services(&workload_labels, &cached_services);

            let containers_json = serde_json::to_value(&merged_containers)
                .context("failed to serialize merged container definition")?;
            let ports_json = serde_json::to_value(&service_ports)
                .context("failed to serialize workload ports definition")?;

            let active_model = kube_app_catalog_workload::ActiveModel {
                id: ActiveValue::Set(workload.id),
                containers: ActiveValue::Set(Json::from(containers_json)),
                ports: ActiveValue::Set(Json::from(ports_json)),
                workload_yaml: ActiveValue::Set(yaml.clone()),
                ..Default::default()
            };

            active_model
                .update(&self.db.conn)
                .await
                .with_context(|| format!("failed to update workload {}", workload.id))?;

            tracing::info!(
                workload_id = %workload.id,
                namespace = %event.namespace,
                resource_name = %event.resource_name,
                "Updated catalog workload containers from cluster event"
            );
        }

        Ok(())
    }

    async fn handle_service_change(&self, event: &ResourceChangeEvent) -> AnyResult<()> {
        if matches!(event.change_type, ResourceChangeType::Deleted) {
            self.db
                .mark_cluster_service_deleted(
                    self.cluster_id,
                    &event.namespace,
                    &event.resource_name,
                    event.timestamp,
                )
                .await?;
            return Ok(());
        }

        let yaml = event
            .resource_yaml
            .as_ref()
            .context("service event missing resource YAML")?;

        let service: Service = serde_yaml::from_str(yaml)?;

        let (selector_json, ports_json, service_type, cluster_ip) = {
            let spec = service.spec.as_ref();
            let selector = spec
                .and_then(|spec| spec.selector.clone())
                .unwrap_or_default();
            let selector_json = json!(selector);

            let ports = spec
                .and_then(|spec| spec.ports.clone())
                .unwrap_or_default()
                .into_iter()
                .map(|port| {
                    let target_port = port.target_port.map(|tp| match tp {
                        k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::Int(v) => {
                            json!(v)
                        }
                        k8s_openapi::apimachinery::pkg::util::intstr::IntOrString::String(s) => {
                            json!(s)
                        }
                    });

                    json!({
                        "name": port.name,
                        "port": port.port,
                        "target_port": target_port,
                        "protocol": port.protocol,
                        "app_protocol": port.app_protocol,
                        "node_port": port.node_port,
                    })
                })
                .collect::<Vec<_>>();
            let ports_json = json!(ports);

            let service_type = spec.and_then(|spec| spec.type_.clone());
            let cluster_ip = spec.and_then(|spec| spec.cluster_ip.clone());

            (selector_json, ports_json, service_type, cluster_ip)
        };

        self.db
            .upsert_cluster_service(
                self.cluster_id,
                &event.namespace,
                &event.resource_name,
                &event.resource_version,
                yaml.clone(),
                selector_json,
                ports_json,
                service_type,
                cluster_ip,
                event.timestamp,
            )
            .await?;

        Ok(())
    }
}

fn workload_kind_for(resource_type: ResourceType) -> Option<KubeWorkloadKind> {
    match resource_type {
        ResourceType::Deployment => Some(KubeWorkloadKind::Deployment),
        ResourceType::StatefulSet => Some(KubeWorkloadKind::StatefulSet),
        ResourceType::DaemonSet => Some(KubeWorkloadKind::DaemonSet),
        ResourceType::ReplicaSet => Some(KubeWorkloadKind::ReplicaSet),
        ResourceType::Job => Some(KubeWorkloadKind::Job),
        ResourceType::CronJob => Some(KubeWorkloadKind::CronJob),
        ResourceType::ConfigMap | ResourceType::Secret | ResourceType::Service => None,
    }
}

struct ExtractedWorkload {
    containers: Vec<KubeContainerInfo>,
    labels: BTreeMap<String, String>,
}

fn extract_workload_from_yaml(
    resource_type: ResourceType,
    yaml: &str,
) -> AnyResult<ExtractedWorkload> {
    match resource_type {
        ResourceType::Deployment => {
            let deployment: Deployment = serde_yaml::from_str(yaml)?;
            let pod_spec = deployment
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("deployment missing pod spec")?;
            let labels = deployment
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok(ExtractedWorkload { containers, labels })
        }
        ResourceType::StatefulSet => {
            let statefulset: StatefulSet = serde_yaml::from_str(yaml)?;
            let pod_spec = statefulset
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("statefulset missing pod spec")?;
            let labels = statefulset
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok(ExtractedWorkload { containers, labels })
        }
        ResourceType::DaemonSet => {
            let daemonset: DaemonSet = serde_yaml::from_str(yaml)?;
            let pod_spec = daemonset
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("daemonset missing pod spec")?;
            let labels = daemonset
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok(ExtractedWorkload { containers, labels })
        }
        ResourceType::ReplicaSet => {
            let replicaset: ReplicaSet = serde_yaml::from_str(yaml)?;
            let pod_spec = replicaset
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.spec.as_ref())
                .context("replicaset missing pod spec")?;
            let labels = replicaset
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok(ExtractedWorkload { containers, labels })
        }
        ResourceType::Job => {
            let job: Job = serde_yaml::from_str(yaml)?;
            let pod_spec = job
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("job missing pod spec")?;
            let labels = job
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok(ExtractedWorkload { containers, labels })
        }
        ResourceType::CronJob => {
            let cron_job: CronJob = serde_yaml::from_str(yaml)?;
            let pod_spec = cron_job
                .spec
                .as_ref()
                .and_then(|s| s.job_template.spec.as_ref())
                .and_then(|s| s.template.spec.as_ref())
                .context("cronjob missing pod spec")?;
            let labels = cron_job
                .spec
                .as_ref()
                .and_then(|s| s.job_template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok(ExtractedWorkload { containers, labels })
        }
        ResourceType::ConfigMap | ResourceType::Secret | ResourceType::Service => {
            Ok(ExtractedWorkload {
                containers: Vec::new(),
                labels: BTreeMap::new(),
            })
        }
    }
}

fn extract_pod_spec_containers(pod_spec: &PodSpec) -> AnyResult<Vec<KubeContainerInfo>> {
    pod_spec
        .containers
        .iter()
        .map(|container| {
            let mut cpu_request = None;
            let mut cpu_limit = None;
            let mut memory_request = None;
            let mut memory_limit = None;

            if let Some(resources) = &container.resources {
                if let Some(requests) = &resources.requests {
                    if let Some(cpu) = requests.get("cpu") {
                        cpu_request = Some(cpu.0.clone());
                    }
                    if let Some(memory) = requests.get("memory") {
                        memory_request = Some(memory.0.clone());
                    }
                }
                if let Some(limits) = &resources.limits {
                    if let Some(cpu) = limits.get("cpu") {
                        cpu_limit = Some(cpu.0.clone());
                    }
                    if let Some(memory) = limits.get("memory") {
                        memory_limit = Some(memory.0.clone());
                    }
                }
            }

            let image = container
                .image
                .clone()
                .ok_or_else(|| anyhow!("container '{}' missing image", container.name))?;

            let ports = container
                .ports
                .as_ref()
                .map(|ports| {
                    ports
                        .iter()
                        .map(|port| KubeContainerPort {
                            name: port.name.clone(),
                            container_port: port.container_port,
                            protocol: port.protocol.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Ok(KubeContainerInfo {
                name: container.name.clone(),
                original_image: image.clone(),
                image: KubeContainerImage::FollowOriginal,
                cpu_request,
                cpu_limit,
                memory_request,
                memory_limit,
                env_vars: Vec::new(),
                original_env_vars: Vec::new(),
                ports,
            })
        })
        .collect()
}

fn merge_containers(
    existing: &Json,
    new_containers: &[KubeContainerInfo],
) -> AnyResult<Vec<KubeContainerInfo>> {
    let existing_containers: Vec<KubeContainerInfo> =
        serde_json::from_value(existing.clone()).unwrap_or_default();
    let mut existing_map: HashMap<String, KubeContainerInfo> = existing_containers
        .into_iter()
        .map(|container| (container.name.clone(), container))
        .collect();

    let mut merged = Vec::new();
    for new_container in new_containers {
        if let Some(mut existing) = existing_map.remove(&new_container.name) {
            existing.original_image = new_container.original_image.clone();
            existing.cpu_request = new_container.cpu_request.clone();
            existing.cpu_limit = new_container.cpu_limit.clone();
            existing.memory_request = new_container.memory_request.clone();
            existing.memory_limit = new_container.memory_limit.clone();
            existing.ports = new_container.ports.clone();
            // Preserve customized image choices; otherwise keep following original.
            if let KubeContainerImage::FollowOriginal = existing.image {
                existing.image = KubeContainerImage::FollowOriginal;
            }
            merged.push(existing);
        } else {
            merged.push(new_container.clone());
        }
    }

    if !existing_map.is_empty() {
        tracing::debug!(
            removed_containers = ?existing_map.keys().collect::<Vec<_>>(),
            "Dropping containers no longer present in source workload"
        );
    }

    Ok(merged)
}

fn ports_from_cached_services(
    workload_labels: &BTreeMap<String, String>,
    services: &[CachedClusterService],
) -> Vec<KubeServicePort> {
    if services.is_empty() {
        return Vec::new();
    }

    let mut ports = Vec::new();
    let mut seen = HashSet::new();

    for service in services {
        if service.selector.is_empty() {
            continue;
        }

        let matches = service
            .selector
            .iter()
            .all(|(key, value)| workload_labels.get(key).map_or(false, |v| v == value));

        if !matches {
            continue;
        }

        for port in &service.ports {
            let key = (port.port, port.target_port, port.protocol.clone());
            if seen.insert(key) {
                ports.push(port.clone());
            }
        }
    }

    ports
}
