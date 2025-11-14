use anyhow::{anyhow, Context as _, Result as AnyResult};
use chrono::{DateTime, Utc};
use futures::future::BoxFuture;
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{PodSpec, Service},
};
use lapdev_common::{
    devbox::DirectTunnelConfig,
    kube::{
        EnvironmentWorkloadStatusEvent, KubeClusterInfo, KubeContainerImage, KubeContainerInfo,
        KubeContainerPort, KubeServicePort, KubeWorkloadKind,
    },
};
use lapdev_db::api::{CachedClusterService, DbApi};
use lapdev_db_entities::{
    kube_app_catalog_workload::{self, Entity as CatalogWorkloadEntity},
    kube_app_catalog_workload_dependency, kube_app_catalog_workload_label, kube_environment,
    kube_environment_workload,
};
use lapdev_kube_rpc::{
    DevboxRouteConfig, KubeClusterRpc, KubeManagerRpcClient, ProxyBranchRouteConfig,
    ProxyRouteAccessLevel, ResourceChangeEvent, ResourceChangeType, ResourceType,
};
use lapdev_rpc::error::ApiError;
use sea_orm::prelude::{DateTimeWithTimeZone, Json};
use sea_orm::{ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter};
use serde_json::json;
use std::convert::TryFrom;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc,
};
use std::{
    collections::{BTreeMap, BTreeSet, HashMap, HashSet},
    net::SocketAddr,
};
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::tunnel::TunnelRegistry;

pub trait DirectConfigProvider: Send + Sync {
    fn request_direct_config(
        &self,
        user_id: Uuid,
        stun_observed_addr: Option<SocketAddr>,
    ) -> BoxFuture<'static, Result<Option<DirectTunnelConfig>, String>>;
}

/// KubeClusterServer is the central server where
/// KubeManager and KubeCli connects to
#[derive(Clone)]
pub struct KubeClusterServer {
    cluster_id: Uuid,
    pub rpc_client: KubeManagerRpcClient,
    db: DbApi,
    kube_cluster_servers: Arc<RwLock<HashMap<Uuid, BTreeMap<u64, KubeClusterServer>>>>,
    generation_counter: Arc<AtomicU64>,
    connection_generation: Arc<RwLock<Option<u64>>>,
    tunnel_registry: Arc<TunnelRegistry>,
    direct_config_provider: Arc<dyn DirectConfigProvider>,
}

impl KubeClusterServer {
    pub fn new(
        cluster_id: Uuid,
        client: KubeManagerRpcClient,
        db: DbApi,
        kube_cluster_servers: Arc<RwLock<HashMap<Uuid, BTreeMap<u64, KubeClusterServer>>>>,
        generation_counter: Arc<AtomicU64>,
        tunnel_registry: Arc<TunnelRegistry>,
        direct_config_provider: Arc<dyn DirectConfigProvider>,
    ) -> Self {
        Self {
            cluster_id,
            rpc_client: client,
            db,
            kube_cluster_servers,
            generation_counter,
            connection_generation: Arc::new(RwLock::new(None)),
            tunnel_registry,
            direct_config_provider,
        }
    }

    pub fn cluster_id(&self) -> Uuid {
        self.cluster_id
    }

    pub async fn register(&self) {
        let generation = self.generation_counter.fetch_add(1, Ordering::SeqCst);
        {
            let mut guard = self.connection_generation.write().await;
            *guard = Some(generation);
        }

        {
            let mut servers = self.kube_cluster_servers.write().await;
            servers
                .entry(self.cluster_id)
                .or_insert_with(BTreeMap::new)
                .insert(generation, self.clone());
        }
        tracing::info!(
            "Registered KubeClusterServer for cluster {}",
            self.cluster_id
        );

        if let Err(err) = self.sync_namespace_watches_from_db().await {
            tracing::warn!(
                cluster_id = %self.cluster_id,
                error = ?err,
                "Failed to send initial namespace watch configuration"
            );
        }
    }

    pub async fn unregister(&self) {
        let generation = { *self.connection_generation.read().await };
        let Some(generation) = generation else {
            tracing::warn!(
                cluster_id = %self.cluster_id,
                "KubeClusterServer unregister called without recorded generation"
            );
            return;
        };

        let mut servers = self.kube_cluster_servers.write().await;
        if let Some(cluster_servers) = servers.get_mut(&self.cluster_id) {
            if cluster_servers.remove(&generation).is_some() {
                tracing::info!(
                    "Unregistered KubeClusterServer for cluster {} generation {}",
                    self.cluster_id,
                    generation
                );
            }

            if cluster_servers.is_empty() {
                servers.remove(&self.cluster_id);
            }
        }
    }

    pub async fn send_namespace_watch_configuration(
        &self,
        namespaces: Vec<String>,
    ) -> AnyResult<()> {
        let namespace_count = namespaces.len();
        tracing::info!(
            cluster_id = %self.cluster_id,
            namespace_count,
            "Sending namespace watch configuration to KubeManager"
        );

        match self
            .rpc_client
            .configure_watches(tarpc::context::current(), namespaces)
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(anyhow!(
                "KubeManager rejected namespace watch configuration: {}",
                err
            )),
            Err(err) => Err(anyhow!(
                "Failed to send namespace watch configuration RPC: {}",
                err
            )),
        }
    }

    async fn fetch_cluster_namespaces(&self) -> AnyResult<Vec<String>> {
        Ok(self
            .db
            .get_cluster_watch_namespaces(self.cluster_id)
            .await?)
    }

    pub async fn sync_namespace_watches_from_db(&self) -> AnyResult<()> {
        let namespaces = self.fetch_cluster_namespaces().await?;
        self.send_namespace_watch_configuration(namespaces).await
    }

    pub async fn add_namespace_watch(&self, namespace: String) -> AnyResult<()> {
        let namespace = namespace.trim().to_string();
        if namespace.is_empty() {
            return Ok(());
        }
        tracing::info!(
            cluster_id = %self.cluster_id,
            namespace = namespace.as_str(),
            "Adding namespace watch via RPC"
        );
        match self
            .rpc_client
            .add_namespace_watch(tarpc::context::current(), namespace)
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(anyhow!("KubeManager rejected namespace watch add: {err}")),
            Err(err) => Err(anyhow!("Failed to add namespace watch RPC: {err}")),
        }
    }

    pub async fn remove_namespace_watch(&self, namespace: String) -> AnyResult<()> {
        let namespace = namespace.trim().to_string();
        if namespace.is_empty() {
            return Ok(());
        }
        tracing::info!(
            cluster_id = %self.cluster_id,
            namespace = namespace.as_str(),
            "Removing namespace watch via RPC"
        );
        match self
            .rpc_client
            .remove_namespace_watch(tarpc::context::current(), namespace)
            .await
        {
            Ok(Ok(())) => Ok(()),
            Ok(Err(err)) => Err(anyhow!(
                "KubeManager rejected namespace watch removal: {err}"
            )),
            Err(err) => Err(anyhow!("Failed to remove namespace watch RPC: {err}")),
        }
    }

    pub async fn set_devbox_routes(
        &self,
        environment_id: Uuid,
        routes: HashMap<Uuid, DevboxRouteConfig>,
    ) -> Result<(), String> {
        match self
            .rpc_client
            .set_devbox_routes(tarpc::context::current(), environment_id, routes)
            .await
        {
            Ok(result) => result,
            Err(err) => Err(format!(
                "Failed to send set_devbox_routes RPC to kube-manager: {}",
                err
            )),
        }
    }

    pub async fn set_devbox_route(
        &self,
        environment_id: Uuid,
        route: DevboxRouteConfig,
    ) -> Result<(), String> {
        match self
            .rpc_client
            .set_devbox_route(tarpc::context::current(), environment_id, route)
            .await
        {
            Ok(result) => result,
            Err(err) => Err(format!(
                "Failed to send set_devbox_route RPC to kube-manager: {}",
                err
            )),
        }
    }

    pub async fn remove_devbox_route(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        match self
            .rpc_client
            .remove_devbox_route(
                tarpc::context::current(),
                environment_id,
                workload_id,
                branch_environment_id,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => Err(format!(
                "Failed to send remove_devbox_route RPC to kube-manager: {}",
                err
            )),
        }
    }

    pub async fn clear_devbox_routes(
        &self,
        environment_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        match self
            .rpc_client
            .clear_devbox_routes(
                tarpc::context::current(),
                environment_id,
                branch_environment_id,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => Err(format!(
                "Failed to send clear_devbox_routes RPC to kube-manager: {}",
                err
            )),
        }
    }

    pub async fn get_devbox_direct_config(
        &self,
        user_id: Uuid,
        environment_id: Uuid,
        namespace: String,
        stun_observed_addr: Option<SocketAddr>,
    ) -> Result<Option<DirectTunnelConfig>, String> {
        match self
            .rpc_client
            .get_devbox_direct_config(
                tarpc::context::current(),
                user_id,
                environment_id,
                namespace,
                stun_observed_addr,
            )
            .await
        {
            Ok(result) => result,
            Err(err) => Err(format!(
                "Failed to send get_devbox_direct_config RPC to kube-manager: {}",
                err
            )),
        }
    }

    pub async fn build_branch_service_route_config(
        &self,
        base_environment: &lapdev_db_entities::kube_environment::Model,
        base_workload_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<Option<ProxyBranchRouteConfig>, ApiError> {
        let workload_labels = self
            .db
            .get_environment_workload_labels(base_workload_id)
            .await
            .map_err(ApiError::from)?;

        let shared_services = self
            .db
            .get_matching_cluster_services(
                base_environment.cluster_id,
                &base_environment.namespace,
                &workload_labels,
            )
            .await
            .map_err(ApiError::from)?;

        if shared_services.is_empty() {
            return Ok(None);
        }

        Ok(Self::build_branch_service_route_config_from_services(
            branch_environment_id,
            &shared_services,
        ))
    }

    fn build_branch_service_route_config_from_services(
        branch_environment_id: Uuid,
        shared_services: &[CachedClusterService],
    ) -> Option<ProxyBranchRouteConfig> {
        let mut service_names = HashMap::new();
        let branch_suffix = format!("-{}", branch_environment_id);

        for service in shared_services {
            let branch_service_name = format!("{}{branch_suffix}", service.name);
            for port in &service.ports {
                match u16::try_from(port.port) {
                    Ok(port_number) => {
                        service_names.insert(port_number, branch_service_name.clone());
                    }
                    Err(_) => {
                        tracing::warn!(
                            "Skipping branch service mapping for {} in env {} due to unsupported port {}",
                            branch_service_name,
                            branch_environment_id,
                            port.port
                        );
                    }
                }
            }
        }

        if service_names.is_empty() {
            tracing::warn!(
                "Found shared services for branch env {} but no valid ports; skipping branch route",
                branch_environment_id
            );
            return None;
        }

        Some(ProxyBranchRouteConfig {
            branch_environment_id,
            service_names,
            headers: HashMap::new(),
            requires_auth: true,
            access_level: ProxyRouteAccessLevel::Personal,
            timeout_ms: None,
            devbox_route: None,
        })
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
                cluster_info.provider,
                cluster_info.region,
                cluster_info.manager_namespace,
            )
            .await
            .map_err(|e| format!("Failed to update cluster info: {}", e))?;

        tracing::info!(
            "Successfully updated cluster {} info in database",
            self.cluster_id
        );

        Ok(())
    }

    async fn heartbeat(self, _context: ::tarpc::context::Context) -> Result<(), String> {
        tracing::trace!("Received heartbeat from cluster {}", self.cluster_id);
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
            ResourceType::ConfigMap => self.handle_dependency_change(&event, "configmap").await,
            ResourceType::Secret => self.handle_dependency_change(&event, "secret").await,
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

    async fn list_branch_service_routes(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
        workload_id: Uuid,
    ) -> Result<Vec<ProxyBranchRouteConfig>, String> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment {}: {}", environment_id, e))?
            .ok_or_else(|| format!("Environment {} not found", environment_id))?;

        if environment.cluster_id != self.cluster_id {
            return Err(format!(
                "Environment {} does not belong to cluster {}",
                environment_id, self.cluster_id
            ));
        }

        let _workload = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(|e| format!("Failed to fetch workload {}: {}", workload_id, e))?
            .ok_or_else(|| format!("Workload {} not found", workload_id))?;

        let workload_labels = self
            .db
            .get_environment_workload_labels(workload_id)
            .await
            .map_err(|e| format!("Failed to fetch labels for workload {}: {}", workload_id, e))?;

        let shared_services = self
            .db
            .get_matching_cluster_services(
                self.cluster_id,
                &environment.namespace,
                &workload_labels,
            )
            .await
            .map_err(|e| {
                format!(
                    "Failed to resolve services for workload {} in namespace {}: {}",
                    workload_id, environment.namespace, e
                )
            })?;

        if shared_services.is_empty() {
            return Ok(Vec::new());
        }

        let branch_workloads = self
            .db
            .get_workloads_by_base_workload_id(workload_id)
            .await
            .map_err(|e| {
                format!(
                    "Failed to fetch workloads for base workload {}: {}",
                    workload_id, e
                )
            })?;

        let mut routes = Vec::new();

        for branch_workload in branch_workloads {
            let branch_env_id = branch_workload.environment_id;

            if branch_env_id == environment_id {
                continue;
            }

            let Some(environment) = self
                .db
                .get_kube_environment(branch_env_id)
                .await
                .map_err(|e| format!("Failed to fetch environment {}: {}", branch_env_id, e))?
            else {
                continue;
            };

            if environment.cluster_id != self.cluster_id {
                continue;
            }

            if let Some(route) = Self::build_branch_service_route_config_from_services(
                branch_env_id,
                &shared_services,
            ) {
                routes.push(route);
            }
        }

        Ok(routes)
    }

    async fn request_direct_config(
        self,
        _context: ::tarpc::context::Context,
        environment_id: Uuid,
        stun_observed_addr: Option<SocketAddr>,
    ) -> Result<Option<DirectTunnelConfig>, String> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| format!("Failed to fetch environment {}: {}", environment_id, e))?
            .ok_or_else(|| format!("Environment {} not found", environment_id))?;

        if environment.cluster_id != self.cluster_id {
            return Err(format!(
                "Environment {} does not belong to cluster {}",
                environment_id, self.cluster_id
            ));
        }

        self.direct_config_provider
            .request_direct_config(environment.user_id, stun_observed_addr)
            .await
    }
}

impl KubeClusterServer {
    async fn handle_workload_change(&self, event: &ResourceChangeEvent) -> AnyResult<()> {
        let Some(workload_kind) = workload_kind_for(event.resource_type) else {
            // Ignore non-workload resources for now.
            return Ok(());
        };

        if matches!(event.change_type, ResourceChangeType::Deleted) {
            self.handle_workload_deleted(event).await?;
            return Ok(());
        }

        let yaml = event
            .resource_yaml
            .as_ref()
            .context("workload event missing resource YAML")?;

        let ExtractedWorkload {
            containers: new_containers,
            pod_labels: workload_labels,
            metadata_labels,
            configmap_refs,
            secret_refs,
            ready_replicas,
            spec_snapshot,
        } = extract_workload_from_yaml(event.resource_type, yaml).with_context(|| {
            format!(
                "failed to parse workload YAML for {}/{} ({:?})",
                event.namespace, event.resource_name, event.resource_type
            )
        })?;

        if new_containers.is_empty() {
            tracing::warn!(
                namespace = %event.namespace,
                resource_name = %event.resource_name,
                "Parsed workload contains no containers; skipping update"
            );
            return Ok(());
        }

        self.sync_catalog_from_workload_event(
            event,
            workload_kind,
            yaml,
            spec_snapshot.as_ref(),
            &workload_labels,
            &new_containers,
            &configmap_refs,
            &secret_refs,
        )
        .await?;

        self.update_environment_workload_ready_replicas(
            &event.namespace,
            &event.resource_name,
            &metadata_labels,
            ready_replicas,
            event.timestamp,
        )
        .await
        .with_context(|| {
            format!(
                "failed to update environment ready replicas for {}/{}",
                event.namespace, event.resource_name
            )
        })?;

        Ok(())
    }

    async fn handle_workload_deleted(&self, event: &ResourceChangeEvent) -> AnyResult<()> {
        let metadata_labels = if let Some(yaml) = event.resource_yaml.as_ref() {
            match extract_workload_from_yaml(event.resource_type, yaml)
                .map(|extracted| extracted.metadata_labels)
            {
                Ok(labels) => labels,
                Err(err) => {
                    tracing::debug!(
                        namespace = %event.namespace,
                        resource_name = %event.resource_name,
                        error = ?err,
                        "Failed to parse workload YAML for delete event; will clear ready replicas using namespace lookup"
                    );
                    BTreeMap::new()
                }
            }
        } else {
            BTreeMap::new()
        };

        self.update_environment_workload_ready_replicas(
            &event.namespace,
            &event.resource_name,
            &metadata_labels,
            None,
            event.timestamp,
        )
        .await
        .with_context(|| {
            format!(
                "failed to clear environment ready replicas for deleted workload {}/{}",
                event.namespace, event.resource_name
            )
        })?;

        tracing::debug!(
            namespace = %event.namespace,
            resource_name = %event.resource_name,
            "Cleared environment workload ready replicas after workload deletion"
        );

        Ok(())
    }

    async fn sync_catalog_from_workload_event(
        &self,
        event: &ResourceChangeEvent,
        workload_kind: KubeWorkloadKind,
        yaml: &str,
        new_spec_snapshot: Option<&serde_json::Value>,
        workload_labels: &BTreeMap<String, String>,
        new_containers: &[KubeContainerInfo],
        configmap_refs: &BTreeSet<String>,
        secret_refs: &BTreeSet<String>,
    ) -> AnyResult<()> {
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
                "Workload not tracked in any catalog; skipping catalog sync update"
            );
            return Ok(());
        }

        let matching_services = self
            .db
            .get_matching_cluster_services(self.cluster_id, &event.namespace, workload_labels)
            .await?;
        let service_ports = ports_from_cached_services(workload_labels, &matching_services);

        let workload_ids: Vec<Uuid> = workloads.iter().map(|w| w.id).collect();
        let existing_state = self.load_existing_catalog_state(&workload_ids).await?;

        let mut workloads_by_catalog: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

        for workload in workloads {
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

            let mut update = Self::compute_catalog_workload_update(
                event,
                &workload,
                new_containers,
                &service_ports,
                yaml,
                new_spec_snapshot,
                workload_labels,
                existing_state.labels.get(&workload.id),
                configmap_refs,
                secret_refs,
                existing_state.dependencies.get(&workload.id),
            )?;

            let requires_bump = update.requires_catalog_bump();

            if update.has_model_updates() {
                let mut active_model = kube_app_catalog_workload::ActiveModel {
                    id: ActiveValue::Set(workload.id),
                    ..Default::default()
                };

                if let Some(containers_json) = update.containers.take() {
                    active_model.containers = ActiveValue::Set(containers_json);
                }
                if let Some(ports_json) = update.ports.take() {
                    active_model.ports = ActiveValue::Set(ports_json);
                }
                if let Some(updated_yaml) = update.workload_yaml.take() {
                    active_model.workload_yaml = ActiveValue::Set(updated_yaml);
                }

                active_model
                    .update(&self.db.conn)
                    .await
                    .with_context(|| format!("failed to update workload {}", workload.id))?;
            }

            if update.labels_changed {
                self.db
                    .replace_workload_labels(
                        workload.id,
                        workload.app_catalog_id,
                        workload.cluster_id,
                        &workload.namespace,
                        workload_labels,
                        event.timestamp.into(),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update workload label mapping for {}",
                            workload.id
                        )
                    })?;
            }

            if update.dependencies_changed {
                self.db
                    .replace_workload_dependencies(
                        workload.id,
                        workload.app_catalog_id,
                        workload.cluster_id,
                        &workload.namespace,
                        configmap_refs,
                        secret_refs,
                        event.timestamp.into(),
                    )
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update workload dependency mapping for {}",
                            workload.id
                        )
                    })?;
            }

            if requires_bump {
                tracing::info!(
                    workload_id = %workload.id,
                    namespace = %event.namespace,
                    resource_name = %event.resource_name,
                    "Recorded catalog workload update from cluster event"
                );

                workloads_by_catalog
                    .entry(workload.app_catalog_id)
                    .or_default()
                    .push(workload.id);
            } else {
                tracing::trace!(
                    workload_id = %workload.id,
                    namespace = %event.namespace,
                    resource_name = %event.resource_name,
                    "No substantive catalog workload change detected; skipping version bump"
                );
            }
        }

        if !workloads_by_catalog.is_empty() {
            let synced_at: DateTimeWithTimeZone = event.timestamp.into();
            for (catalog_id, workload_ids) in workloads_by_catalog {
                let new_version = self
                    .db
                    .bump_app_catalog_sync_version(catalog_id, synced_at.clone())
                    .await
                    .with_context(|| {
                        format!(
                            "failed to bump sync version for catalog {} after workload update",
                            catalog_id
                        )
                    })?;
                self.db
                    .update_catalog_workload_versions(&workload_ids, new_version)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update workload sync version for catalog {}",
                            catalog_id
                        )
                    })?;
            }
        }

        Ok(())
    }

    async fn load_existing_catalog_state(
        &self,
        workload_ids: &[Uuid],
    ) -> AnyResult<ExistingCatalogState> {
        let mut state = ExistingCatalogState::default();
        if workload_ids.is_empty() {
            return Ok(state);
        }

        let id_list: Vec<Uuid> = workload_ids.to_vec();

        let label_rows = kube_app_catalog_workload_label::Entity::find()
            .filter(kube_app_catalog_workload_label::Column::WorkloadId.is_in(id_list.clone()))
            .filter(kube_app_catalog_workload_label::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await?;

        for row in label_rows {
            state
                .labels
                .entry(row.workload_id)
                .or_default()
                .insert(row.label_key, row.label_value);
        }

        let dependency_rows = kube_app_catalog_workload_dependency::Entity::find()
            .filter(kube_app_catalog_workload_dependency::Column::WorkloadId.is_in(id_list))
            .filter(kube_app_catalog_workload_dependency::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await?;

        for row in dependency_rows {
            let entry = state
                .dependencies
                .entry(row.workload_id)
                .or_insert_with(|| (BTreeSet::new(), BTreeSet::new()));

            match row.resource_type.to_ascii_lowercase().as_str() {
                "configmap" => {
                    entry.0.insert(row.resource_name);
                }
                "secret" => {
                    entry.1.insert(row.resource_name);
                }
                _ => {}
            }
        }

        Ok(state)
    }

    fn compute_catalog_workload_update(
        event: &ResourceChangeEvent,
        workload: &kube_app_catalog_workload::Model,
        new_containers: &[KubeContainerInfo],
        service_ports: &[KubeServicePort],
        yaml: &str,
        new_spec_snapshot: Option<&serde_json::Value>,
        new_labels: &BTreeMap<String, String>,
        existing_labels: Option<&BTreeMap<String, String>>,
        new_configmaps: &BTreeSet<String>,
        new_secrets: &BTreeSet<String>,
        existing_dependencies: Option<&(BTreeSet<String>, BTreeSet<String>)>,
    ) -> AnyResult<CatalogWorkloadUpdate> {
        let merged_containers = merge_containers(&workload.containers, new_containers)
            .with_context(|| {
                format!(
                    "failed to merge container definitions for workload {}",
                    workload.id
                )
            })?;

        let containers_json = Json::from(
            serde_json::to_value(&merged_containers)
                .context("failed to serialize merged container definition")?,
        );
        let containers_changed = containers_json != workload.containers;

        let ports_json = Json::from(
            serde_json::to_value(service_ports)
                .context("failed to serialize workload ports definition")?,
        );
        let ports_changed = ports_json != workload.ports;

        let (stored_spec_snapshot, stored_spec_failed) =
            match spec_section_from_yaml(&workload.workload_yaml) {
                Ok(snapshot) => (snapshot, false),
                Err(err) => {
                    tracing::debug!(
                        workload_id = %workload.id,
                        namespace = %event.namespace,
                        resource_name = %event.resource_name,
                        error = ?err,
                        "Failed to extract stored spec from workload YAML"
                    );
                    (None, true)
                }
            };

        let spec_changed = if stored_spec_failed {
            true
        } else {
            match (new_spec_snapshot, stored_spec_snapshot.as_ref()) {
                (Some(new_spec), Some(existing_spec)) => new_spec != existing_spec,
                (None, None) => false,
                _ => true,
            }
        };

        let labels_changed = match existing_labels {
            Some(current_labels) => current_labels != new_labels,
            None => !new_labels.is_empty(),
        };

        let dependencies_changed = match existing_dependencies {
            Some((existing_configmaps, existing_secrets)) => {
                existing_configmaps != new_configmaps || existing_secrets != new_secrets
            }
            None => !(new_configmaps.is_empty() && new_secrets.is_empty()),
        };

        Ok(CatalogWorkloadUpdate {
            containers: if containers_changed {
                Some(containers_json)
            } else {
                None
            },
            ports: if ports_changed {
                Some(ports_json)
            } else {
                None
            },
            workload_yaml: if spec_changed {
                Some(yaml.to_owned())
            } else {
                None
            },
            containers_changed,
            ports_changed,
            spec_changed,
            labels_changed,
            dependencies_changed,
        })
    }

    async fn update_environment_workload_ready_replicas(
        &self,
        namespace: &str,
        resource_name: &str,
        metadata_labels: &BTreeMap<String, String>,
        ready_replicas: Option<i32>,
        event_timestamp: DateTime<Utc>,
    ) -> AnyResult<()> {
        let branch_environment_id = metadata_labels
            .get("lapdev.io/branch-environment-id")
            .and_then(|value| Uuid::parse_str(value).ok());

        let environment = if let Some(env_id) = branch_environment_id {
            let environment = kube_environment::Entity::find_by_id(env_id)
                .filter(kube_environment::Column::DeletedAt.is_null())
                .one(&self.db.conn)
                .await?;

            match environment {
                Some(env) if env.cluster_id == self.cluster_id => Some(env),
                Some(env) => {
                    tracing::debug!(
                        environment_id = %env_id,
                        event_cluster = %self.cluster_id,
                        stored_cluster = %env.cluster_id,
                        "Environment from workload labels belongs to different cluster; skipping ready replica update"
                    );
                    None
                }
                None => {
                    tracing::debug!(
                        environment_id = %env_id,
                        namespace,
                        "Environment referenced by workload labels not found; skipping ready replica update"
                    );
                    None
                }
            }
        } else {
            kube_environment::Entity::find()
                .filter(kube_environment::Column::ClusterId.eq(self.cluster_id))
                .filter(kube_environment::Column::Namespace.eq(namespace.to_string()))
                .filter(kube_environment::Column::DeletedAt.is_null())
                .one(&self.db.conn)
                .await?
        };

        let Some(environment) = environment else {
            return Ok(());
        };

        let mut workload = if let Some(base_workload_id) = metadata_labels
            .get("lapdev.base-workload-id")
            .and_then(|value| {
                Uuid::parse_str(value)
                    .map_err(|err| {
                        tracing::debug!(
                            label_value = value,
                            error = ?err,
                            "Invalid lapdev.base-workload-id label; ignoring"
                        );
                        err
                    })
                    .ok()
            }) {
            kube_environment_workload::Entity::find()
                .filter(kube_environment_workload::Column::EnvironmentId.eq(environment.id))
                .filter(
                    kube_environment_workload::Column::BaseWorkloadId.eq(Some(base_workload_id)),
                )
                .filter(kube_environment_workload::Column::DeletedAt.is_null())
                .one(&self.db.conn)
                .await?
        } else {
            None
        };

        if workload.is_none() {
            workload = kube_environment_workload::Entity::find()
                .filter(kube_environment_workload::Column::EnvironmentId.eq(environment.id))
                .filter(kube_environment_workload::Column::Name.eq(resource_name.to_string()))
                .filter(kube_environment_workload::Column::DeletedAt.is_null())
                .one(&self.db.conn)
                .await?;
        }

        let Some(workload) = workload else {
            tracing::trace!(
                environment_id = %environment.id,
                namespace,
                resource_name,
                "No matching environment workload found while updating ready replicas"
            );
            return Ok(());
        };

        if workload.ready_replicas == ready_replicas {
            return Ok(());
        }

        let workload_id = workload.id;
        let organization_id = environment.organization_id;
        let environment_id = environment.id;
        let mut active_model: kube_environment_workload::ActiveModel = workload.into();
        active_model.ready_replicas = ActiveValue::Set(ready_replicas);
        active_model.update(&self.db.conn).await?;

        let status_event = EnvironmentWorkloadStatusEvent {
            organization_id,
            environment_id,
            workload_id,
            ready_replicas,
            updated_at: event_timestamp,
        };

        self.db
            .publish_environment_workload_status_event(&status_event)
            .await;

        tracing::debug!(
            environment_id = %environment_id,
            workload_id = %workload_id,
            namespace,
            resource_name,
            ready_replicas = ?ready_replicas,
            "Updated environment workload ready replicas"
        );

        Ok(())
    }

    async fn handle_service_change(&self, event: &ResourceChangeEvent) -> AnyResult<()> {
        if matches!(event.change_type, ResourceChangeType::Deleted) {
            let selector_map = self
                .db
                .get_service_selector_map(self.cluster_id, &event.namespace, &event.resource_name)
                .await?;
            self.db
                .mark_cluster_service_deleted(
                    self.cluster_id,
                    &event.namespace,
                    &event.resource_name,
                    event.timestamp,
                )
                .await?;
            if let Some(selector_map) = selector_map {
                self.reconcile_workloads_for_selector(
                    &event.namespace,
                    &event.resource_name,
                    &selector_map,
                    event.timestamp,
                )
                .await?;
            }
            return Ok(());
        }

        let yaml = event
            .resource_yaml
            .as_ref()
            .context("service event missing resource YAML")?;

        let service: Service = serde_yaml::from_str(yaml)?;

        let (selector_map, selector_json, ports_json, service_type, cluster_ip) = {
            let spec = service.spec.as_ref();
            let selector = spec
                .and_then(|spec| spec.selector.clone())
                .unwrap_or_default();
            let selector_map: BTreeMap<String, String> = selector.into_iter().collect();
            let selector_json = json!(selector_map);

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

            (
                selector_map,
                selector_json,
                ports_json,
                service_type,
                cluster_ip,
            )
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

        self.reconcile_workloads_for_selector(
            &event.namespace,
            &event.resource_name,
            &selector_map,
            event.timestamp,
        )
        .await
    }

    async fn reconcile_workloads_for_selector(
        &self,
        namespace: &str,
        resource_name: &str,
        selector_map: &BTreeMap<String, String>,
        observed_at: chrono::DateTime<chrono::Utc>,
    ) -> AnyResult<()> {
        if selector_map.is_empty() {
            return Ok(());
        }

        let matching_workload_ids = self
            .db
            .find_workloads_matching_selector(self.cluster_id, namespace, selector_map)
            .await
            .with_context(|| {
                format!(
                    "failed to resolve workloads matching service selector for {}",
                    resource_name
                )
            })?;

        if matching_workload_ids.is_empty() {
            return Ok(());
        }

        let workloads = CatalogWorkloadEntity::find()
            .filter(kube_app_catalog_workload::Column::Id.is_in(matching_workload_ids.clone()))
            .filter(kube_app_catalog_workload::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await
            .with_context(|| {
                format!(
                    "failed querying catalog workloads for service selector {}/{}",
                    namespace, resource_name
                )
            })?;

        if workloads.is_empty() {
            return Ok(());
        }

        let label_rows = kube_app_catalog_workload_label::Entity::find()
            .filter(
                kube_app_catalog_workload_label::Column::WorkloadId.is_in(matching_workload_ids),
            )
            .filter(kube_app_catalog_workload_label::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await?
            .into_iter()
            .fold(
                HashMap::<Uuid, BTreeMap<String, String>>::new(),
                |mut acc, row| {
                    acc.entry(row.workload_id)
                        .or_default()
                        .insert(row.label_key, row.label_value);
                    acc
                },
            );

        let mut workloads_by_catalog: HashMap<Uuid, Vec<Uuid>> = HashMap::new();

        for workload in workloads {
            let labels = label_rows.get(&workload.id).cloned().unwrap_or_default();
            let matching_services = self
                .db
                .get_matching_cluster_services(self.cluster_id, namespace, &labels)
                .await?;
            let service_ports = ports_from_cached_services(&labels, &matching_services);
            let ports_json = Json::from(serde_json::to_value(&service_ports)?);

            if ports_json != workload.ports {
                let active_model = kube_app_catalog_workload::ActiveModel {
                    id: ActiveValue::Set(workload.id),
                    ports: ActiveValue::Set(ports_json),
                    ..Default::default()
                };

                active_model.update(&self.db.conn).await.with_context(|| {
                    format!("failed to update workload ports for {}", workload.id)
                })?;

                workloads_by_catalog
                    .entry(workload.app_catalog_id)
                    .or_default()
                    .push(workload.id);
            }
        }

        if !workloads_by_catalog.is_empty() {
            let synced_at: DateTimeWithTimeZone = observed_at.into();
            for (catalog_id, workload_ids) in workloads_by_catalog {
                let new_version = self
                    .db
                    .bump_app_catalog_sync_version(catalog_id, synced_at.clone())
                    .await
                    .with_context(|| {
                        format!(
                            "failed to bump sync version for catalog {} after service update",
                            catalog_id
                        )
                    })?;
                self.db
                    .update_catalog_workload_versions(&workload_ids, new_version)
                    .await
                    .with_context(|| {
                        format!(
                            "failed to update workload sync version for catalog {}",
                            catalog_id
                        )
                    })?;
            }
        }

        Ok(())
    }

    async fn handle_dependency_change(
        &self,
        event: &ResourceChangeEvent,
        resource_type: &str,
    ) -> AnyResult<()> {
        let workloads = self
            .db
            .find_workloads_by_dependency(
                self.cluster_id,
                &event.namespace,
                resource_type,
                &event.resource_name,
            )
            .await?;

        if workloads.is_empty() {
            return Ok(());
        }

        let mut workloads_by_catalog: HashMap<Uuid, Vec<Uuid>> = HashMap::new();
        for (workload_id, catalog_id) in workloads {
            workloads_by_catalog
                .entry(catalog_id)
                .or_default()
                .push(workload_id);
        }

        let synced_at: DateTimeWithTimeZone = event.timestamp.into();
        for (catalog_id, workload_ids) in workloads_by_catalog {
            let new_version = self
                .db
                .bump_app_catalog_sync_version(catalog_id, synced_at.clone())
                .await
                .with_context(|| {
                    format!(
                        "failed to bump sync version for catalog {} after {} change",
                        catalog_id, resource_type
                    )
                })?;

            self.db
                .update_catalog_workload_versions(&workload_ids, new_version)
                .await
                .with_context(|| {
                    format!(
                        "failed to update workload sync version for catalog {}",
                        catalog_id
                    )
                })?;
        }

        Ok(())
    }
}

#[derive(Default)]
struct ExistingCatalogState {
    labels: HashMap<Uuid, BTreeMap<String, String>>,
    dependencies: HashMap<Uuid, (BTreeSet<String>, BTreeSet<String>)>,
}

struct CatalogWorkloadUpdate {
    containers: Option<Json>,
    ports: Option<Json>,
    workload_yaml: Option<String>,
    containers_changed: bool,
    ports_changed: bool,
    spec_changed: bool,
    labels_changed: bool,
    dependencies_changed: bool,
}

impl CatalogWorkloadUpdate {
    fn has_model_updates(&self) -> bool {
        self.containers.is_some() || self.ports.is_some() || self.workload_yaml.is_some()
    }

    fn requires_catalog_bump(&self) -> bool {
        self.containers_changed
            || self.ports_changed
            || self.spec_changed
            || self.labels_changed
            || self.dependencies_changed
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
    pod_labels: BTreeMap<String, String>,
    metadata_labels: BTreeMap<String, String>,
    configmap_refs: BTreeSet<String>,
    secret_refs: BTreeSet<String>,
    ready_replicas: Option<i32>,
    spec_snapshot: Option<serde_json::Value>,
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
            let pod_labels = deployment
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let metadata_labels = deployment.metadata.labels.clone().unwrap_or_default();
            let ready_replicas = deployment
                .status
                .as_ref()
                .and_then(|status| status.ready_replicas);
            let spec_snapshot = deployment
                .spec
                .as_ref()
                .map(|spec| serde_json::to_value(spec))
                .transpose()
                .context("failed to serialize deployment spec")?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            let (configmap_refs, secret_refs) = extract_pod_spec_dependencies(pod_spec);
            Ok(ExtractedWorkload {
                containers,
                pod_labels,
                metadata_labels,
                configmap_refs,
                secret_refs,
                ready_replicas,
                spec_snapshot,
            })
        }
        ResourceType::StatefulSet => {
            let statefulset: StatefulSet = serde_yaml::from_str(yaml)?;
            let pod_spec = statefulset
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("statefulset missing pod spec")?;
            let pod_labels = statefulset
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let metadata_labels = statefulset.metadata.labels.clone().unwrap_or_default();
            let ready_replicas = statefulset
                .status
                .as_ref()
                .and_then(|status| status.ready_replicas);
            let spec_snapshot = statefulset
                .spec
                .as_ref()
                .map(|spec| serde_json::to_value(spec))
                .transpose()
                .context("failed to serialize statefulset spec")?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            let (configmap_refs, secret_refs) = extract_pod_spec_dependencies(pod_spec);
            Ok(ExtractedWorkload {
                containers,
                pod_labels,
                metadata_labels,
                configmap_refs,
                secret_refs,
                ready_replicas,
                spec_snapshot,
            })
        }
        ResourceType::DaemonSet => {
            let daemonset: DaemonSet = serde_yaml::from_str(yaml)?;
            let pod_spec = daemonset
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("daemonset missing pod spec")?;
            let pod_labels = daemonset
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let metadata_labels = daemonset.metadata.labels.clone().unwrap_or_default();
            let ready_replicas = daemonset.status.as_ref().map(|status| status.number_ready);
            let spec_snapshot = daemonset
                .spec
                .as_ref()
                .map(|spec| serde_json::to_value(spec))
                .transpose()
                .context("failed to serialize daemonset spec")?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            let (configmap_refs, secret_refs) = extract_pod_spec_dependencies(pod_spec);
            Ok(ExtractedWorkload {
                containers,
                pod_labels,
                metadata_labels,
                configmap_refs,
                secret_refs,
                ready_replicas,
                spec_snapshot,
            })
        }
        ResourceType::ReplicaSet => {
            let replicaset: ReplicaSet = serde_yaml::from_str(yaml)?;
            let pod_spec = replicaset
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.spec.as_ref())
                .context("replicaset missing pod spec")?;
            let pod_labels = replicaset
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let metadata_labels = replicaset.metadata.labels.clone().unwrap_or_default();
            let ready_replicas = replicaset
                .status
                .as_ref()
                .and_then(|status| status.ready_replicas);
            let spec_snapshot = replicaset
                .spec
                .as_ref()
                .map(|spec| serde_json::to_value(spec))
                .transpose()
                .context("failed to serialize replicaset spec")?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            let (configmap_refs, secret_refs) = extract_pod_spec_dependencies(pod_spec);
            Ok(ExtractedWorkload {
                containers,
                pod_labels,
                metadata_labels,
                configmap_refs,
                secret_refs,
                ready_replicas,
                spec_snapshot,
            })
        }
        ResourceType::Job => {
            let job: Job = serde_yaml::from_str(yaml)?;
            let pod_spec = job
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .context("job missing pod spec")?;
            let pod_labels = job
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let metadata_labels = job.metadata.labels.clone().unwrap_or_default();
            let ready_replicas = job.status.as_ref().and_then(|status| status.succeeded);
            let spec_snapshot = job
                .spec
                .as_ref()
                .map(|spec| serde_json::to_value(spec))
                .transpose()
                .context("failed to serialize job spec")?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            let (configmap_refs, secret_refs) = extract_pod_spec_dependencies(pod_spec);
            Ok(ExtractedWorkload {
                containers,
                pod_labels,
                metadata_labels,
                configmap_refs,
                secret_refs,
                ready_replicas,
                spec_snapshot,
            })
        }
        ResourceType::CronJob => {
            let cron_job: CronJob = serde_yaml::from_str(yaml)?;
            let pod_spec = cron_job
                .spec
                .as_ref()
                .and_then(|s| s.job_template.spec.as_ref())
                .and_then(|s| s.template.spec.as_ref())
                .context("cronjob missing pod spec")?;
            let pod_labels = cron_job
                .spec
                .as_ref()
                .and_then(|s| s.job_template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let metadata_labels = cron_job.metadata.labels.clone().unwrap_or_default();
            let containers = extract_pod_spec_containers(pod_spec)?;
            let (configmap_refs, secret_refs) = extract_pod_spec_dependencies(pod_spec);
            let spec_snapshot = cron_job
                .spec
                .as_ref()
                .map(|spec| serde_json::to_value(spec))
                .transpose()
                .context("failed to serialize cronjob spec")?;
            Ok(ExtractedWorkload {
                containers,
                pod_labels,
                metadata_labels,
                configmap_refs,
                secret_refs,
                ready_replicas: Some(1),
                spec_snapshot,
            })
        }
        ResourceType::ConfigMap | ResourceType::Secret | ResourceType::Service => {
            Ok(ExtractedWorkload {
                containers: Vec::new(),
                pod_labels: BTreeMap::new(),
                metadata_labels: BTreeMap::new(),
                configmap_refs: BTreeSet::new(),
                secret_refs: BTreeSet::new(),
                ready_replicas: None,
                spec_snapshot: None,
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

fn extract_pod_spec_dependencies(pod_spec: &PodSpec) -> (BTreeSet<String>, BTreeSet<String>) {
    let mut configmaps = BTreeSet::new();
    let mut secrets = BTreeSet::new();

    let insert_trimmed = |set: &mut BTreeSet<String>, raw: &str| {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            set.insert(trimmed.to_owned());
        }
    };

    let mut process_env_sources =
        |env: &Option<Vec<k8s_openapi::api::core::v1::EnvVar>>,
         env_from: &Option<Vec<k8s_openapi::api::core::v1::EnvFromSource>>| {
            if let Some(envs) = env {
                for item in envs {
                    if let Some(value_from) = &item.value_from {
                        if let Some(cfg) = &value_from.config_map_key_ref {
                            insert_trimmed(&mut configmaps, &cfg.name);
                        }
                        if let Some(sec) = &value_from.secret_key_ref {
                            insert_trimmed(&mut secrets, &sec.name);
                        }
                    }
                }
            }

            if let Some(env_from) = env_from {
                for source in env_from {
                    if let Some(cfg) = &source.config_map_ref {
                        insert_trimmed(&mut configmaps, &cfg.name);
                    }
                    if let Some(sec) = &source.secret_ref {
                        insert_trimmed(&mut secrets, &sec.name);
                    }
                }
            }
        };

    for container in &pod_spec.containers {
        process_env_sources(&container.env, &container.env_from);
    }

    if let Some(init_containers) = pod_spec.init_containers.as_ref() {
        for container in init_containers {
            process_env_sources(&container.env, &container.env_from);
        }
    }

    if let Some(ephemeral) = pod_spec.ephemeral_containers.as_ref() {
        for container in ephemeral {
            process_env_sources(&container.env, &container.env_from);
        }
    }

    if let Some(volumes) = pod_spec.volumes.as_ref() {
        for volume in volumes {
            if let Some(cfg) = &volume.config_map {
                insert_trimmed(&mut configmaps, &cfg.name);
            }
            if let Some(secret) = &volume.secret {
                if let Some(name) = secret.secret_name.as_ref() {
                    insert_trimmed(&mut secrets, name);
                }
            }
            if let Some(projected) = &volume.projected {
                if let Some(sources) = projected.sources.as_ref() {
                    for source in sources {
                        if let Some(cfg) = &source.config_map {
                            insert_trimmed(&mut configmaps, &cfg.name);
                        }
                        if let Some(secret) = &source.secret {
                            insert_trimmed(&mut secrets, &secret.name);
                        }
                    }
                }
            }
        }
    }

    (configmaps, secrets)
}

fn spec_section_from_yaml(yaml: &str) -> AnyResult<Option<serde_json::Value>> {
    let document: serde_yaml::Value = serde_yaml::from_str(yaml)?;

    match document.get("spec") {
        Some(spec_value) => Ok(Some(serde_json::to_value(spec_value)?)),
        None => Ok(None),
    }
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
            let original_target = port.original_target_port.or(port.target_port);
            let key = (port.port, original_target, port.protocol.clone());
            if seen.insert(key) {
                ports.push(port.clone());
            }
        }
    }

    ports
}

#[cfg(test)]
mod tests;
