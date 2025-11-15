use chrono::Utc;
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{Pod, PodSpec, Service},
};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use lapdev_common::{
    kube::{
        KubeAppCatalogWorkload, KubeContainerImage, KubeEnvironment,
        KubeEnvironmentDashboardSummary, KubeEnvironmentStatus, KubeEnvironmentSyncStatus,
        KubeServiceDetails, KubeServicePort, KubeServiceWithYaml, KubeWorkloadDetails,
        KubeWorkloadKind, PagePaginationParams, PaginatedInfo, PaginatedResult,
    },
    utils::rand_string,
};
use lapdev_db::api::NewEnvironmentWorkload;
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::{KubeWorkloadYamlOnly, KubeWorkloadsWithResources, NamespacedResourceKind};
use lapdev_rpc::error::ApiError;
use sea_orm::{
    prelude::Json, sea_query::Expr, ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait,
    QueryFilter, TransactionTrait,
};
use serde_yaml;
use std::{
    collections::{HashMap, HashSet},
    str::FromStr,
    time::{Duration, Instant},
};
use tracing::{error, warn};
use uuid::Uuid;

use crate::environment_events::EnvironmentLifecycleEvent;

use super::{
    app_catalog::SidecarInjectionContext,
    resources::{set_cronjob_suspend, set_daemonset_paused, set_workload_replicas},
    workload::{build_branch_service_selector, rename_service_yaml, rename_workload_yaml},
    EnvironmentNamespaceKind, KubeController,
};

impl KubeController {
    pub(super) async fn generate_unique_namespace(
        &self,
        _cluster_id: Uuid,
        kind: EnvironmentNamespaceKind,
    ) -> Result<String, ApiError> {
        let prefix = match kind {
            EnvironmentNamespaceKind::Personal => "lapdev-personal",
            EnvironmentNamespaceKind::Shared => "lapdev-shared",
            EnvironmentNamespaceKind::Branch => "lapdev-branch",
        };

        Ok(format!("{prefix}-{}", rand_string(12)))
    }

    pub async fn get_all_kube_environments(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        search: Option<String>,
        is_shared: bool,
        is_branch: bool,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, ApiError> {
        let pagination = pagination.unwrap_or_default();

        let (environments_with_catalogs_and_clusters, total_count) = self
            .db
            .get_all_kube_environments_paginated(
                org_id,
                user_id,
                search,
                is_shared,
                is_branch,
                Some(pagination.clone()),
            )
            .await
            .map_err(ApiError::from)?;

        let kube_environments = environments_with_catalogs_and_clusters
            .into_iter()
            .filter_map(|(env, catalog, cluster, base_environment_name)| {
                let catalog = catalog?;
                let cluster = cluster?;
                let catalog_sync_version = env.catalog_sync_version;
                let status = KubeEnvironmentStatus::from_str(&env.status)
                    .unwrap_or(KubeEnvironmentStatus::Creating);
                let last_catalog_synced_at = env.last_catalog_synced_at.map(|dt| dt.to_string());
                let paused_at = env.paused_at.map(|dt| dt.to_string());
                let resumed_at = env.resumed_at.map(|dt| dt.to_string());
                let catalog_update_available = catalog.sync_version > catalog_sync_version;

                Some(KubeEnvironment {
                    id: env.id,
                    name: env.name,
                    namespace: env.namespace,
                    app_catalog_id: env.app_catalog_id,
                    app_catalog_name: catalog.name,
                    cluster_id: env.cluster_id,
                    cluster_name: cluster.name,
                    status,
                    created_at: env.created_at.to_string(),
                    user_id: env.user_id,
                    is_shared: env.is_shared,
                    base_environment_id: env.base_environment_id,
                    base_environment_name,
                    catalog_sync_version,
                    last_catalog_synced_at,
                    paused_at,
                    resumed_at,
                    catalog_update_available,
                    catalog_last_sync_actor_id: catalog.last_sync_actor_id,
                    sync_status: KubeEnvironmentSyncStatus::from_str(&env.sync_status)
                        .unwrap_or(KubeEnvironmentSyncStatus::Idle),
                })
            })
            .collect();

        let total_pages = (total_count + pagination.page_size - 1) / pagination.page_size;

        Ok(PaginatedResult {
            data: kube_environments,
            pagination_info: PaginatedInfo {
                total_count,
                page: pagination.page,
                page_size: pagination.page_size,
                total_pages,
            },
        })
    }

    pub async fn get_environment_dashboard_summary(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        recent_limit: Option<usize>,
    ) -> Result<KubeEnvironmentDashboardSummary, ApiError> {
        let limit = recent_limit.unwrap_or(5);
        self.db
            .get_environment_dashboard_summary(org_id, user_id, limit)
            .await
            .map_err(ApiError::from)
    }

    pub async fn get_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<KubeEnvironment, ApiError> {
        // Get the environment from database
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        // Check authorization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Get related catalog and cluster info
        let catalog = self
            .db
            .get_app_catalog(environment.app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        let cluster = self
            .db
            .get_kube_cluster(environment.cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        // Get base environment name if this is a branch environment
        let base_environment_name = if let Some(base_env_id) = environment.base_environment_id {
            self.db
                .get_kube_environment(base_env_id)
                .await
                .map_err(ApiError::from)?
                .map(|base_env| base_env.name)
        } else {
            None
        };

        let catalog_update_available = catalog.sync_version > environment.catalog_sync_version;

        let status = KubeEnvironmentStatus::from_str(&environment.status)
            .unwrap_or(KubeEnvironmentStatus::Creating);

        Ok(KubeEnvironment {
            id: environment.id,
            user_id: environment.user_id,
            name: environment.name,
            namespace: environment.namespace,
            app_catalog_id: environment.app_catalog_id,
            app_catalog_name: catalog.name,
            cluster_id: environment.cluster_id,
            cluster_name: cluster.name,
            status,
            created_at: environment
                .created_at
                .format("%Y-%m-%d %H:%M:%S%.f %z")
                .to_string(),
            is_shared: environment.is_shared,
            base_environment_id: environment.base_environment_id,
            base_environment_name,
            catalog_sync_version: environment.catalog_sync_version,
            last_catalog_synced_at: environment.last_catalog_synced_at.map(|dt| dt.to_string()),
            paused_at: environment.paused_at.map(|dt| dt.to_string()),
            resumed_at: environment.resumed_at.map(|dt| dt.to_string()),
            catalog_last_sync_actor_id: catalog.last_sync_actor_id,
            catalog_update_available,
            sync_status: KubeEnvironmentSyncStatus::from_str(&environment.sync_status)
                .unwrap_or(KubeEnvironmentSyncStatus::Idle),
        })
    }

    /// Prepare workload details for environment creation by copying from catalog workloads
    pub(super) fn prepare_workload_details_from_catalog(
        workloads: Vec<lapdev_common::kube::KubeAppCatalogWorkload>,
        namespace: &str,
        env_workload_ids: &HashMap<Uuid, Uuid>,
    ) -> Result<Vec<NewEnvironmentWorkload>, ApiError> {
        workloads
            .into_iter()
            .map(|workload| {
                if workload.workload_yaml.trim().is_empty() {
                    return Err(ApiError::InvalidRequest(format!(
                        "Workload {} from catalog is missing YAML",
                        workload.name
                    )));
                }
                let env_workload_id = env_workload_ids.get(&workload.id).ok_or_else(|| {
                    ApiError::InvalidRequest(format!(
                        "Missing generated workload id for catalog workload {}",
                        workload.name
                    ))
                })?;
                let workload_yaml = workload.workload_yaml.clone();
                let mut containers = workload.containers;
                for container in &mut containers {
                    // Preserve the original environment variables
                    container.original_env_vars = container.env_vars.clone();
                    container.env_vars.clear();

                    // If the app catalog has a customized image, use it as the original_image
                    // for the new environment (so the environment starts from the customized state)
                    match &container.image {
                        KubeContainerImage::Custom(custom_image) => {
                            container.original_image = custom_image.clone();
                            container.image = KubeContainerImage::FollowOriginal;
                        }
                        KubeContainerImage::FollowOriginal => {
                            // Keep the current original_image and FollowOriginal setting
                        }
                    }
                }
                Ok(NewEnvironmentWorkload {
                    id: *env_workload_id,
                    details: KubeWorkloadDetails {
                        name: workload.name,
                        namespace: namespace.to_string(),
                        kind: workload.kind,
                        containers,
                        ports: workload.ports,
                        workload_yaml,
                        base_workload_id: None,
                    },
                })
            })
            .collect()
    }

    /// Build the manifest set used for pause/resume.
    ///
    /// We snapshot workload YAML when the environment is created or synced. This method:
    /// 1. Pulls that stored manifest from the DB (fails if missing—we require it for new feature).
    /// 2. Wraps each YAML string into the appropriate `KubeWorkloadYamlOnly` variant.
    /// 3. Loads stored service YAML so we redeploy exactly what was saved.
    ///
    /// No catalog fallback: avoiding divergence and ensuring pause/resume replay the original manifest.
    async fn assemble_environment_workloads(
        &self,
        environment: &lapdev_db_entities::kube_environment::Model,
    ) -> Result<KubeWorkloadsWithResources, ApiError> {
        let stored_workloads = self
            .db
            .get_environment_workloads(environment.id)
            .await
            .map_err(ApiError::from)?;

        if stored_workloads.is_empty()
            || stored_workloads
                .iter()
                .any(|w| w.workload_yaml.trim().is_empty())
        {
            return Err(ApiError::InvalidRequest(
                "Environment workloads are missing stored manifests".to_string(),
            ));
        }

        let mut workloads = Vec::new();
        for workload in stored_workloads {
            let kind = KubeWorkloadKind::from_str(&workload.kind).map_err(|_| {
                ApiError::InvalidRequest(format!(
                    "Invalid workload kind {} for environment {}",
                    workload.kind, environment.id
                ))
            })?;
            workloads.push(Self::wrap_workload_yaml(kind, workload.workload_yaml));
        }

        Ok(KubeWorkloadsWithResources {
            workloads,
            services: HashMap::new(),
            configmaps: HashMap::new(),
            secrets: HashMap::new(),
        })
    }

    /// Build KubeEnvironment response from database model
    fn build_environment_response(
        created_env: lapdev_db_entities::kube_environment::Model,
        app_catalog_name: String,
        cluster_name: String,
        base_environment_name: Option<String>,
    ) -> lapdev_common::kube::KubeEnvironment {
        let status = KubeEnvironmentStatus::from_str(&created_env.status)
            .unwrap_or(KubeEnvironmentStatus::Creating);
        lapdev_common::kube::KubeEnvironment {
            id: created_env.id,
            user_id: created_env.user_id,
            name: created_env.name,
            namespace: created_env.namespace,
            status,
            is_shared: created_env.is_shared,
            app_catalog_id: created_env.app_catalog_id,
            app_catalog_name,
            cluster_id: created_env.cluster_id,
            cluster_name,
            created_at: created_env.created_at.to_string(),
            base_environment_id: created_env.base_environment_id,
            base_environment_name,
            catalog_sync_version: created_env.catalog_sync_version,
            last_catalog_synced_at: created_env.last_catalog_synced_at.map(|dt| dt.to_string()),
            paused_at: created_env.paused_at.map(|dt| dt.to_string()),
            resumed_at: created_env.resumed_at.map(|dt| dt.to_string()),
            catalog_update_available: false,
            catalog_last_sync_actor_id: None,
            sync_status: KubeEnvironmentSyncStatus::from_str(&created_env.sync_status)
                .unwrap_or(KubeEnvironmentSyncStatus::Idle),
        }
    }

    fn wrap_workload_yaml(kind: KubeWorkloadKind, yaml: String) -> KubeWorkloadYamlOnly {
        match kind {
            KubeWorkloadKind::Deployment => KubeWorkloadYamlOnly::Deployment(yaml),
            KubeWorkloadKind::StatefulSet => KubeWorkloadYamlOnly::StatefulSet(yaml),
            KubeWorkloadKind::DaemonSet => KubeWorkloadYamlOnly::DaemonSet(yaml),
            KubeWorkloadKind::ReplicaSet => KubeWorkloadYamlOnly::ReplicaSet(yaml),
            KubeWorkloadKind::Pod => KubeWorkloadYamlOnly::Pod(yaml),
            KubeWorkloadKind::Job => KubeWorkloadYamlOnly::Job(yaml),
            KubeWorkloadKind::CronJob => KubeWorkloadYamlOnly::CronJob(yaml),
        }
    }

    /// Authorize environment deletion
    async fn authorize_environment_deletion(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<lapdev_db_entities::kube_environment::Model, ApiError> {
        // Get the environment to check ownership
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        // Check authorization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a shared environment, check for depending branch environments
        if environment.is_shared {
            let has_branches = self
                .db
                .check_environment_has_branches(environment_id)
                .await
                .map_err(ApiError::from)?;

            if has_branches {
                return Err(ApiError::InvalidRequest(
                    "Cannot delete shared environment: it has active branch environments. Please delete them first.".to_string()
                ));
            }
        }

        Ok(environment)
    }

    /// Notify devbox-proxy about branch environment deletion
    async fn notify_branch_environment_deletion(
        &self,
        rpc_client: &lapdev_kube_rpc::KubeManagerRpcClient,
        base_env_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), ApiError> {
        match rpc_client
            .remove_branch_environment(tarpc::context::current(), base_env_id, environment_id)
            .await
        {
            Ok(Ok(())) => {
                tracing::info!(
                    "Successfully notified devbox-proxy about branch environment {} deletion",
                    environment_id
                );
                Ok(())
            }
            Ok(Err(e)) => {
                if e.contains("No devbox-proxy registered for base environment") {
                    tracing::warn!(
                        base_environment_id = %base_env_id,
                        branch_environment_id = %environment_id,
                        error = %e,
                        "No devbox-proxy registered for base environment; skipping branch deletion notification"
                    );
                    return Ok(());
                }
                tracing::error!(
                    "Failed to notify devbox-proxy about branch environment {} deletion: {}",
                    environment_id,
                    e
                );
                Err(ApiError::InvalidRequest(format!(
                    "Failed to notify devbox-proxy about branch environment deletion: {e}"
                )))
            }
            Err(e) => {
                tracing::error!(
                    "RPC call failed when notifying about branch environment {} deletion: {}",
                    environment_id,
                    e
                );
                Err(ApiError::InvalidRequest(format!(
                    "Connection error while notifying devbox-proxy: {e}"
                )))
            }
        }
    }

    /// Destroy environment resources via RPC
    async fn destroy_environment_resources(
        &self,
        rpc_client: &lapdev_kube_rpc::KubeManagerRpcClient,
        environment: &lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), ApiError> {
        let mut ctx = tarpc::context::current();
        ctx.deadline = Instant::now() + Duration::from_secs(300);

        match rpc_client
            .destroy_environment(ctx, environment.id, environment.namespace.clone())
            .await
        {
            Ok(Ok(())) => {
                tracing::info!(
                    "Successfully deleted resources for environment {} in namespace {}",
                    environment.id,
                    environment.namespace
                );
                Ok(())
            }
            Ok(Err(e)) => {
                tracing::error!(
                    "KubeManager error when deleting environment {}: {}",
                    environment.id,
                    e
                );
                Err(ApiError::InvalidRequest(format!(
                    "Failed to delete environment resources: {e}"
                )))
            }
            Err(e) => {
                tracing::error!(
                    "Connection error when deleting environment {}: {}",
                    environment.id,
                    e
                );
                Err(ApiError::InvalidRequest(format!(
                    "Failed to communicate with KubeManager to delete environment: {e}"
                )))
            }
        }
    }

    async fn destroy_branch_environment_resources(
        &self,
        rpc_client: &lapdev_kube_rpc::KubeManagerRpcClient,
        environment: &lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), ApiError> {
        let mut resources: Vec<(NamespacedResourceKind, String)> = Vec::new();

        let branch_namespace = environment.namespace.clone();

        let branch_workloads = self
            .db
            .get_environment_workloads(environment.id)
            .await
            .map_err(ApiError::from)?;
        for workload in branch_workloads {
            let kind = KubeWorkloadKind::from_str(&workload.kind).map_err(|_| {
                ApiError::InvalidRequest(format!(
                    "Invalid workload kind {} in branch environment",
                    workload.kind
                ))
            })?;
            resources.push((Self::workload_kind_to_resource_kind(&kind), workload.name));
        }

        let branch_services = self
            .db
            .get_environment_services(environment.id)
            .await
            .map_err(ApiError::from)?;
        for service in branch_services {
            resources.push((NamespacedResourceKind::Service, service.name));
        }

        let mut ctx = tarpc::context::current();
        ctx.deadline = Instant::now() + Duration::from_secs(300);
        match rpc_client
            .delete_namespaced_resources(ctx, branch_namespace.clone(), resources)
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    environment_id = %environment.id,
                    error = %err,
                    "KubeManager refused to delete some branch resources"
                );
            }
            Err(err) => {
                tracing::warn!(
                    environment_id = %environment.id,
                    error = ?err,
                    "RPC error while deleting branch resources"
                );
            }
        }

        Ok(())
    }

    pub async fn delete_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), ApiError> {
        // Authorize deletion
        let environment = self
            .authorize_environment_deletion(org_id, user_id, environment_id)
            .await?;

        // Get RPC client for cluster operations
        let rpc_client = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for this cluster; cannot delete environment"
                        .to_string(),
                )
            })?
            .rpc_client
            .clone();

        // If this is a branch environment, notify the base environment's devbox-proxy
        if let Some(base_env_id) = environment.base_environment_id {
            self.notify_branch_environment_deletion(&rpc_client, base_env_id, environment_id)
                .await?;
        }

        // Mark as deleting immediately
        self.update_environment_status(environment_id, KubeEnvironmentStatus::Deleting, None, None)
            .await?;

        // Destroy environment resources in Kubernetes asynchronously
        let controller = self.clone();
        let environment_clone = environment.clone();
        let rpc_client_clone = rpc_client.clone();

        tokio::spawn(async move {
            let env_id = environment_clone.id;
            let destroy_result = if environment_clone.base_environment_id.is_some() {
                controller
                    .destroy_branch_environment_resources(&rpc_client_clone, &environment_clone)
                    .await
            } else {
                controller
                    .destroy_environment_resources(&rpc_client_clone, &environment_clone)
                    .await
            };

            match destroy_result {
                Ok(_) => {
                    match controller
                        .db
                        .delete_kube_environment(env_id)
                        .await
                        .map_err(ApiError::from)
                    {
                        Ok(_) => {
                            if environment_clone.base_environment_id.is_none() {
                                match rpc_client_clone
                                    .remove_namespace_watch(
                                        tarpc::context::current(),
                                        environment_clone.namespace.clone(),
                                    )
                                    .await
                                {
                                    Ok(Ok(())) => {}
                                    Ok(Err(err)) => warn!(
                                        cluster_id = %environment_clone.cluster_id,
                                        namespace = %environment_clone.namespace,
                                        error = %err,
                                        "Failed to remove namespace watch after environment deletion"
                                    ),
                                    Err(err) => warn!(
                                        cluster_id = %environment_clone.cluster_id,
                                        namespace = %environment_clone.namespace,
                                        error = ?err,
                                        "RPC error while removing namespace watch after environment deletion"
                                    ),
                                }
                            }

                            let event = EnvironmentLifecycleEvent {
                                organization_id: environment_clone.organization_id,
                                environment_id: env_id,
                                status: KubeEnvironmentStatus::Deleted,
                                paused_at: environment_clone
                                    .paused_at
                                    .as_ref()
                                    .map(|dt| dt.to_string()),
                                resumed_at: environment_clone
                                    .resumed_at
                                    .as_ref()
                                    .map(|dt| dt.to_string()),
                                updated_at: Utc::now(),
                            };
                            controller.publish_environment_event(event).await;
                        }
                        Err(err) => {
                            error!(
                                environment_id = %env_id,
                                error = ?err,
                                "failed to delete environment record after resource cleanup"
                            );
                            let _ = controller
                                .update_environment_status(
                                    env_id,
                                    KubeEnvironmentStatus::Error,
                                    None,
                                    None,
                                )
                                .await;
                        }
                    }
                }
                Err(err) => {
                    error!(
                        environment_id = %env_id,
                        error = ?err,
                        "failed to cleanup environment resources"
                    );
                    let _ = controller
                        .update_environment_status(env_id, KubeEnvironmentStatus::Error, None, None)
                        .await;
                }
            }
        });

        Ok(())
    }

    pub async fn pause_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), ApiError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        if environment.is_shared {
            return Err(ApiError::InvalidRequest(
                "Pause is not yet supported for shared environments".to_string(),
            ));
        }

        if environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        let status = KubeEnvironmentStatus::from_str(&environment.status)
            .unwrap_or(KubeEnvironmentStatus::Creating);

        match status {
            KubeEnvironmentStatus::Running | KubeEnvironmentStatus::PauseFailed => {} // allowed transitions
            KubeEnvironmentStatus::Paused => {
                return Err(ApiError::InvalidRequest(
                    "Environment is already paused".to_string(),
                ))
            }
            _ => {
                return Err(ApiError::InvalidRequest(format!(
                    "Environment cannot be paused while it is in status {}",
                    status
                )))
            }
        }

        self.update_environment_status(environment.id, KubeEnvironmentStatus::Pausing, None, None)
            .await?;

        let controller = self.clone();
        tokio::spawn(async move {
            if let Err(err) = controller.run_pause_environment(environment.id).await {
                error!(
                    environment_id = %environment.id,
                    error = ?err,
                    "Pause environment orchestration failed"
                );
                let _ = controller
                    .update_environment_status(
                        environment.id,
                        KubeEnvironmentStatus::PauseFailed,
                        None,
                        None,
                    )
                    .await;
            }
        });

        Ok(())
    }

    pub async fn resume_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), ApiError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        if environment.is_shared {
            return Err(ApiError::InvalidRequest(
                "Resume is not yet supported for shared environments".to_string(),
            ));
        }

        if environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        let status = KubeEnvironmentStatus::from_str(&environment.status)
            .unwrap_or(KubeEnvironmentStatus::Creating);

        match status {
            KubeEnvironmentStatus::Paused
            | KubeEnvironmentStatus::PauseFailed
            | KubeEnvironmentStatus::ResumeFailed => {}
            KubeEnvironmentStatus::Running => {
                return Err(ApiError::InvalidRequest(
                    "Environment is already running".to_string(),
                ))
            }
            _ => {
                return Err(ApiError::InvalidRequest(format!(
                    "Environment cannot be resumed while it is in status {}",
                    status
                )))
            }
        }

        self.update_environment_status(environment.id, KubeEnvironmentStatus::Resuming, None, None)
            .await?;

        let controller = self.clone();
        tokio::spawn(async move {
            if let Err(err) = controller.run_resume_environment(environment.id).await {
                error!(
                    environment_id = %environment.id,
                    error = ?err,
                    "Resume environment orchestration failed"
                );
                let _ = controller
                    .update_environment_status(
                        environment.id,
                        KubeEnvironmentStatus::ResumeFailed,
                        None,
                        None,
                    )
                    .await;
            }
        });

        Ok(())
    }

    /// Validate and get app catalog and cluster for environment creation
    async fn validate_environment_creation(
        &self,
        org_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        is_shared: bool,
    ) -> Result<
        (
            lapdev_db_entities::kube_app_catalog::Model,
            lapdev_db_entities::kube_cluster::Model,
        ),
        ApiError,
    > {
        // Verify app catalog belongs to the organization
        let app_catalog = self
            .db
            .get_app_catalog(app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if app_catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Check if the cluster allows deployments for the requested environment type
        if is_shared && !cluster.can_deploy_shared {
            return Err(ApiError::InvalidRequest(
                "Shared deployments are not allowed on this cluster".to_string(),
            ));
        }

        if !is_shared && !cluster.can_deploy_personal {
            return Err(ApiError::InvalidRequest(
                "Personal deployments are not allowed on this cluster".to_string(),
            ));
        }

        Ok((app_catalog, cluster))
    }

    pub async fn create_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        is_shared: bool,
    ) -> Result<lapdev_common::kube::KubeEnvironment, ApiError> {
        let name = Self::normalize_environment_name(name)?;

        // Validate catalog and cluster
        let (app_catalog, cluster) = self
            .validate_environment_creation(org_id, app_catalog_id, cluster_id, is_shared)
            .await?;

        let server = self.require_cluster_server(cluster_id).await?;
        let workloads = self.fetch_catalog_workloads(&app_catalog).await?;
        let namespace = self
            .generate_unique_namespace(cluster_id, Self::namespace_kind(is_shared))
            .await?;
        let environment_id = Uuid::new_v4();
        let auth_token = rand_string(32);
        let environment_namespace = namespace.clone();
        let (workloads_with_resources, workload_details) = self
            .prepare_catalog_workloads(
                &app_catalog,
                &cluster,
                workloads,
                environment_id,
                &namespace,
                &auth_token,
            )
            .await?;
        let services_map = workloads_with_resources.services.clone();

        let created_env = self
            .persist_environment_creation(
                environment_id,
                org_id,
                user_id,
                app_catalog_id,
                cluster_id,
                name,
                namespace,
                is_shared,
                app_catalog.sync_version,
                workload_details,
                services_map,
                auth_token,
            )
            .await?;

        // Ensure kube-manager begins watching the new namespace immediately.
        if let Err(err) = server
            .add_namespace_watch(environment_namespace.clone())
            .await
        {
            warn!(
                cluster_id = %cluster_id,
                namespace = environment_namespace,
                error = ?err,
                "Failed to add namespace watch for new environment"
            );
        }

        self.spawn_environment_deployment(server, created_env.clone(), workloads_with_resources);

        Ok(Self::build_environment_response(
            created_env,
            app_catalog.name,
            cluster.name,
            None,
        ))
    }

    fn normalize_environment_name(name: String) -> Result<String, ApiError> {
        let trimmed = name.trim();
        if trimmed.is_empty() {
            return Err(ApiError::InvalidRequest(
                "Environment name cannot be empty".to_string(),
            ));
        }
        Ok(trimmed.to_string())
    }

    fn namespace_kind(is_shared: bool) -> EnvironmentNamespaceKind {
        if is_shared {
            EnvironmentNamespaceKind::Shared
        } else {
            EnvironmentNamespaceKind::Personal
        }
    }

    async fn require_cluster_server(
        &self,
        cluster_id: Uuid,
    ) -> Result<KubeClusterServer, ApiError> {
        self.get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })
    }

    async fn fetch_catalog_workloads(
        &self,
        app_catalog: &lapdev_db_entities::kube_app_catalog::Model,
    ) -> Result<Vec<KubeAppCatalogWorkload>, ApiError> {
        let workloads = self
            .db
            .get_app_catalog_workloads(app_catalog.id)
            .await
            .map_err(ApiError::from)?;

        if workloads.is_empty() {
            return Err(ApiError::InvalidRequest(format!(
                "No workloads found for app catalog '{}'",
                app_catalog.name
            )));
        }

        Ok(workloads)
    }

    async fn prepare_catalog_workloads(
        &self,
        app_catalog: &lapdev_db_entities::kube_app_catalog::Model,
        cluster: &lapdev_db_entities::kube_cluster::Model,
        workloads: Vec<KubeAppCatalogWorkload>,
        environment_id: Uuid,
        namespace: &str,
        auth_token: &str,
    ) -> Result<(KubeWorkloadsWithResources, Vec<NewEnvironmentWorkload>), ApiError> {
        self.get_catalog_workloads_with_yaml_from_db(
            app_catalog.cluster_id,
            workloads,
            &SidecarInjectionContext {
                environment_id,
                namespace,
                auth_token,
                manager_namespace: cluster.manager_namespace.as_deref(),
            },
        )
        .await
    }

    async fn persist_environment_creation(
        &self,
        environment_id: Uuid,
        org_id: Uuid,
        user_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
        is_shared: bool,
        catalog_sync_version: i64,
        workload_details: Vec<NewEnvironmentWorkload>,
        services_map: HashMap<String, KubeServiceWithYaml>,
        auth_token: String,
    ) -> Result<lapdev_db_entities::kube_environment::Model, ApiError> {
        match self
            .db
            .create_kube_environment(
                environment_id,
                org_id,
                user_id,
                app_catalog_id,
                cluster_id,
                name,
                namespace,
                KubeEnvironmentStatus::Creating.to_string(),
                is_shared,
                catalog_sync_version,
                None,
                workload_details,
                services_map,
                auth_token,
            )
            .await
        {
            Ok(env) => Ok(env),
            Err(db_err) => {
                if let Some(sea_orm::SqlErr::UniqueConstraintViolation(constraint)) =
                    db_err.sql_err()
                {
                    if constraint == "kube_environment_app_cluster_namespace_unique_idx" {
                        return Err(ApiError::InvalidRequest(
                            "This app catalog is already deployed to the specified namespace in this cluster".to_string(),
                        ));
                    }
                    if constraint == "kube_environment_cluster_namespace_unique_idx" {
                        return Err(ApiError::InternalError(
                            "Namespace allocation conflict. Please retry environment creation."
                                .to_string(),
                        ));
                    }
                }
                Err(ApiError::from(anyhow::Error::from(db_err)))
            }
        }
    }

    fn spawn_environment_deployment(
        &self,
        server: KubeClusterServer,
        created_env: lapdev_db_entities::kube_environment::Model,
        workloads_with_resources: KubeWorkloadsWithResources,
    ) {
        let controller = self.clone();
        let server_clone = server.clone();
        let env_for_task = created_env.clone();

        tokio::spawn(async move {
            match controller
                .deploy_environment_resources(
                    &server_clone,
                    &env_for_task,
                    workloads_with_resources,
                    None,
                )
                .await
            {
                Ok(_) => {
                    if let Err(err) = controller
                        .update_environment_status(
                            env_for_task.id,
                            KubeEnvironmentStatus::Running,
                            None,
                            None,
                        )
                        .await
                    {
                        error!(
                            environment_id = %env_for_task.id,
                            error = ?err,
                            "failed to mark environment as running after creation"
                        );
                    }
                }
                Err(err) => {
                    error!(
                        environment_id = %env_for_task.id,
                        error = ?err,
                        "environment creation deployment failed"
                    );
                    if let Err(status_err) = controller
                        .update_environment_status(
                            env_for_task.id,
                            KubeEnvironmentStatus::Error,
                            None,
                            None,
                        )
                        .await
                    {
                        error!(
                            environment_id = %env_for_task.id,
                            error = ?status_err,
                            "failed to mark environment as errored after deployment failure"
                        );
                    }
                }
            }
        });
    }

    /// Validate base environment for branch creation
    async fn validate_base_environment(
        &self,
        org_id: Uuid,
        base_environment_id: Uuid,
    ) -> Result<
        (
            lapdev_db_entities::kube_environment::Model,
            lapdev_db_entities::kube_cluster::Model,
            lapdev_db_entities::kube_app_catalog::Model,
        ),
        ApiError,
    > {
        // Get the base environment
        let base_environment = self
            .db
            .get_kube_environment(base_environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Base environment not found".to_string()))?;

        // Verify base environment belongs to the same organization
        if base_environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Verify base environment is shared (only shared environments can be used as base)
        if !base_environment.is_shared {
            return Err(ApiError::InvalidRequest(
                "Only shared environments can be used as base environments".to_string(),
            ));
        }

        // Verify base environment is not itself a branch environment
        if base_environment.base_environment_id.is_some() {
            return Err(ApiError::InvalidRequest(
                "Cannot create a branch from another branch environment".to_string(),
            ));
        }

        // Get the cluster
        let cluster = self
            .db
            .get_kube_cluster(base_environment.cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        // Get app catalog
        let app_catalog = self
            .db
            .get_app_catalog(base_environment.app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        Ok((base_environment, cluster, app_catalog))
    }

    fn strip_sidecar_proxy_from_yaml(
        kind: &KubeWorkloadKind,
        yaml: &str,
    ) -> Result<String, ApiError> {
        match kind {
            KubeWorkloadKind::Deployment => {
                let mut deployment: Deployment = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse deployment YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(spec) = deployment.spec.as_mut() {
                    if let Some(pod_spec) = spec.template.spec.as_mut() {
                        Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                    }
                }
                serde_yaml::to_string(&deployment).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize deployment YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
            KubeWorkloadKind::StatefulSet => {
                let mut statefulset: StatefulSet = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse statefulset YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(spec) = statefulset.spec.as_mut() {
                    if let Some(pod_spec) = spec.template.spec.as_mut() {
                        Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                    }
                }
                serde_yaml::to_string(&statefulset).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize statefulset YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
            KubeWorkloadKind::DaemonSet => {
                let mut daemonset: DaemonSet = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse daemonset YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(spec) = daemonset.spec.as_mut() {
                    if let Some(pod_spec) = spec.template.spec.as_mut() {
                        Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                    }
                }
                serde_yaml::to_string(&daemonset).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize daemonset YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
            KubeWorkloadKind::ReplicaSet => {
                let mut replicaset: ReplicaSet = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse replicaset YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(spec) = replicaset.spec.as_mut() {
                    if let Some(template) = spec.template.as_mut() {
                        if let Some(pod_spec) = template.spec.as_mut() {
                            Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                        }
                    }
                }
                serde_yaml::to_string(&replicaset).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize replicaset YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
            KubeWorkloadKind::Pod => {
                let mut pod: Pod = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse pod YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(pod_spec) = pod.spec.as_mut() {
                    Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                }
                serde_yaml::to_string(&pod).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize pod YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
            KubeWorkloadKind::Job => {
                let mut job: Job = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse job YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(spec) = job.spec.as_mut() {
                    if let Some(pod_spec) = spec.template.spec.as_mut() {
                        Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                    }
                }
                serde_yaml::to_string(&job).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize job YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
            KubeWorkloadKind::CronJob => {
                let mut cronjob: CronJob = serde_yaml::from_str(yaml).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to parse cronjob YAML while stripping lapdev-sidecar-proxy: {err}"
                    ))
                })?;
                if let Some(spec) = cronjob.spec.as_mut() {
                    if let Some(job_spec) = spec.job_template.spec.as_mut() {
                        if let Some(pod_spec) = job_spec.template.spec.as_mut() {
                            Self::strip_sidecar_proxy_from_pod_spec(pod_spec);
                        }
                    }
                }
                serde_yaml::to_string(&cronjob).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize cronjob YAML after stripping lapdev-sidecar-proxy: {err}"
                    ))
                })
            }
        }
    }

    fn strip_sidecar_proxy_from_pod_spec(pod_spec: &mut PodSpec) {
        const SIDECAR_NAME: &str = "lapdev-sidecar-proxy";
        pod_spec
            .containers
            .retain(|container| container.name != SIDECAR_NAME);
        if let Some(init_containers) = pod_spec.init_containers.as_mut() {
            init_containers.retain(|container| container.name != SIDECAR_NAME);
        }
    }

    fn restore_service_target_ports(
        service_yaml: String,
        ports: &mut [KubeServicePort],
    ) -> Result<String, ApiError> {
        let mut service: Service = serde_yaml::from_str(&service_yaml).map_err(|err| {
            ApiError::InvalidRequest(format!(
                "Failed to parse service YAML while restoring lapdev-sidecar proxy targets: {err}"
            ))
        })?;

        let mut target_map: HashMap<(Option<String>, i32), i32> = HashMap::new();
        for port in ports.iter_mut() {
            if let Some(original_target) = port.original_target_port {
                port.target_port = Some(original_target);
                target_map.insert((port.name.clone(), port.port), original_target);
            }
        }

        if let Some(spec) = service.spec.as_mut() {
            if let Some(spec_ports) = spec.ports.as_mut() {
                for spec_port in spec_ports.iter_mut() {
                    let key = (spec_port.name.clone(), spec_port.port);
                    if let Some(original) = target_map
                        .get(&key)
                        .or_else(|| target_map.get(&(None, spec_port.port)))
                    {
                        spec_port.target_port = Some(IntOrString::Int(*original));
                    }
                }
            }
        }

        serde_yaml::to_string(&service).map_err(|err| {
            ApiError::InvalidRequest(format!(
                "Failed to serialize service YAML after restoring lapdev-sidecar proxy targets: {err}"
            ))
        })
    }

    fn branch_resource_name(name: &str, branch_environment_id: Uuid) -> String {
        let env_suffix = branch_environment_id.to_string();
        if name.ends_with(&env_suffix) {
            name.to_string()
        } else {
            format!("{}-{}", name, branch_environment_id)
        }
    }

    fn strip_branch_suffix(name: &str, branch_environment_id: Uuid) -> String {
        let suffix = format!("-{}", branch_environment_id);
        if name.ends_with(&suffix) {
            name[..name.len() - suffix.len()].to_string()
        } else {
            name.to_string()
        }
    }

    fn build_branch_workload_details(
        workload: &lapdev_common::kube::KubeEnvironmentWorkload,
        namespace: &str,
        branch_environment_id: Uuid,
    ) -> Result<(NewEnvironmentWorkload, String), ApiError> {
        let kind: KubeWorkloadKind = workload.kind.parse().map_err(|_| {
            ApiError::InvalidRequest(format!(
                "Invalid workload kind {} in base environment",
                workload.kind
            ))
        })?;

        if workload.workload_yaml.trim().is_empty() {
            return Err(ApiError::InvalidRequest(format!(
                "Base workload {} is missing YAML",
                workload.name
            )));
        }

        let branch_name = Self::branch_resource_name(&workload.name, branch_environment_id);
        let sanitized_yaml = Self::strip_sidecar_proxy_from_yaml(&kind, &workload.workload_yaml)?;
        let mut workload_yaml = Self::wrap_workload_yaml(kind.clone(), sanitized_yaml);
        let selector = build_branch_service_selector(&branch_name);
        rename_workload_yaml(&mut workload_yaml, &branch_name, &selector)?;
        let workload_yaml_string = Self::workload_yaml_to_string(&workload_yaml);

        let mut containers = workload.containers.clone();
        for container in &mut containers {
            container.original_env_vars = container.env_vars.clone();
            container.env_vars.clear();

            match &container.image {
                KubeContainerImage::Custom(custom_image) => {
                    container.original_image = custom_image.clone();
                    container.image = KubeContainerImage::FollowOriginal;
                }
                KubeContainerImage::FollowOriginal => {}
            }
        }

        let env_workload_id = Uuid::new_v4();
        let details = KubeWorkloadDetails {
            name: branch_name.clone(),
            namespace: namespace.to_string(),
            kind,
            containers,
            ports: workload.ports.clone(),
            workload_yaml: workload_yaml_string,
            base_workload_id: Some(workload.id),
        };

        Ok((
            NewEnvironmentWorkload {
                id: env_workload_id,
                details,
            },
            branch_name,
        ))
    }

    fn service_targets_workload(
        service: &lapdev_common::kube::KubeEnvironmentService,
        base_workload_name: &str,
        branch_workload_name: &str,
    ) -> bool {
        if service.name == base_workload_name || service.name == branch_workload_name {
            return true;
        }
        service
            .selector
            .values()
            .any(|value| value == base_workload_name || value == branch_workload_name)
    }

    fn build_branch_service_entry(
        service: &lapdev_common::kube::KubeEnvironmentService,
        branch_environment_id: Uuid,
        branch_workload_name: &str,
    ) -> Result<lapdev_common::kube::KubeServiceWithYaml, ApiError> {
        let branch_service_name = Self::branch_resource_name(&service.name, branch_environment_id);
        let branch_selector = build_branch_service_selector(branch_workload_name);
        let renamed_yaml =
            rename_service_yaml(&service.yaml, &branch_service_name, &branch_selector)?;
        let mut ports = service.ports.clone();
        let restored_yaml = Self::restore_service_target_ports(renamed_yaml, &mut ports)?;
        Ok(lapdev_common::kube::KubeServiceWithYaml {
            yaml: restored_yaml,
            details: KubeServiceDetails {
                name: branch_service_name,
                ports,
                selector: branch_selector,
            },
        })
    }

    pub(super) async fn ensure_branch_workload_override(
        &self,
        environment: &lapdev_db_entities::kube_environment::Model,
        base_workload: &lapdev_common::kube::KubeEnvironmentWorkload,
    ) -> Result<lapdev_common::kube::KubeEnvironmentWorkload, ApiError> {
        environment.base_environment_id.ok_or_else(|| {
            ApiError::InvalidRequest("Branch environment missing base environment".to_string())
        })?;

        if let Some(existing) = self
            .db
            .get_branch_workload_override(base_workload.id, environment.id)
            .await
            .map_err(ApiError::from)?
        {
            return Ok(existing);
        }

        let (new_workload, branch_workload_name) = Self::build_branch_workload_details(
            base_workload,
            &environment.namespace,
            environment.id,
        )?;

        let inserted = self
            .db
            .insert_environment_workload(
                environment.id,
                new_workload,
                environment.catalog_sync_version,
            )
            .await
            .map_err(ApiError::from)?;

        self.ensure_branch_services_for_workload(
            environment,
            &base_workload.name,
            &branch_workload_name,
        )
        .await?;

        self.db
            .get_environment_workload(inserted.id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| {
                ApiError::InternalError(
                    "Failed to load newly created branch workload from database".to_string(),
                )
            })
    }

    async fn ensure_branch_services_for_workload(
        &self,
        environment: &lapdev_db_entities::kube_environment::Model,
        base_workload_name: &str,
        branch_workload_name: &str,
    ) -> Result<(), ApiError> {
        let base_environment_id = environment.base_environment_id.ok_or_else(|| {
            ApiError::InvalidRequest("Branch environment missing base environment".to_string())
        })?;

        let base_services = self
            .db
            .get_environment_services(base_environment_id)
            .await
            .map_err(ApiError::from)?;
        let existing_services = self
            .db
            .get_environment_services(environment.id)
            .await
            .map_err(ApiError::from)?;
        let mut existing_names: HashSet<String> = HashSet::new();
        for svc in existing_services {
            existing_names.insert(svc.name.clone());
            existing_names.insert(Self::strip_branch_suffix(&svc.name, environment.id));
        }

        for service in base_services {
            if !Self::service_targets_workload(&service, base_workload_name, branch_workload_name) {
                continue;
            }

            let base_service_name = service.name.clone();
            let branch_service_name =
                Self::branch_resource_name(&base_service_name, environment.id);
            if existing_names.contains(&base_service_name)
                || existing_names.contains(&branch_service_name)
            {
                continue;
            }

            let branch_entry =
                Self::build_branch_service_entry(&service, environment.id, branch_workload_name)?;
            let inserted_name = branch_entry.details.name.clone();
            self.db
                .insert_environment_service(environment.id, &environment.namespace, branch_entry)
                .await
                .map_err(ApiError::from)?;
            existing_names.insert(base_service_name);
            existing_names.insert(inserted_name);
        }

        Ok(())
    }

    fn workload_kind_to_resource_kind(kind: &KubeWorkloadKind) -> NamespacedResourceKind {
        match kind {
            KubeWorkloadKind::Deployment => NamespacedResourceKind::Deployment,
            KubeWorkloadKind::StatefulSet => NamespacedResourceKind::StatefulSet,
            KubeWorkloadKind::DaemonSet => NamespacedResourceKind::DaemonSet,
            KubeWorkloadKind::ReplicaSet => NamespacedResourceKind::ReplicaSet,
            KubeWorkloadKind::Pod => NamespacedResourceKind::Pod,
            KubeWorkloadKind::Job => NamespacedResourceKind::Job,
            KubeWorkloadKind::CronJob => NamespacedResourceKind::CronJob,
        }
    }

    fn workload_yaml_to_string(workload: &KubeWorkloadYamlOnly) -> String {
        match workload {
            KubeWorkloadYamlOnly::Deployment(yaml)
            | KubeWorkloadYamlOnly::StatefulSet(yaml)
            | KubeWorkloadYamlOnly::DaemonSet(yaml)
            | KubeWorkloadYamlOnly::ReplicaSet(yaml)
            | KubeWorkloadYamlOnly::Pod(yaml)
            | KubeWorkloadYamlOnly::Job(yaml)
            | KubeWorkloadYamlOnly::CronJob(yaml) => yaml.clone(),
        }
    }

    /// Notify devbox-proxy about new branch environment
    async fn notify_branch_environment_creation(
        &self,
        base_environment_id: Uuid,
        cluster_id: Uuid,
        created_env: &lapdev_db_entities::kube_environment::Model,
    ) {
        if let Some(server) = self.get_random_kube_cluster_server(cluster_id).await {
            let branch_info = lapdev_kube_rpc::BranchEnvironmentInfo {
                environment_id: created_env.id,
                auth_token: created_env.auth_token.clone(),
                namespace: created_env.namespace.clone(),
            };

            match server
                .rpc_client
                .add_branch_environment(tarpc::context::current(), base_environment_id, branch_info)
                .await
            {
                Ok(Ok(())) => {
                    tracing::info!(
                        "Successfully notified devbox-proxy about new branch environment {}",
                        created_env.id
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        "Failed to notify devbox-proxy about new branch environment {}: {}",
                        created_env.id,
                        e
                    );
                }
                Err(e) => {
                    tracing::error!(
                        "RPC call failed when notifying about new branch environment {}: {}",
                        created_env.id,
                        e
                    );
                }
            }
        } else {
            tracing::warn!(
                "No connected KubeManager for cluster {} - branch environment {} not registered with devbox-proxy",
                cluster_id,
                created_env.id
            );
        }
    }

    pub async fn create_branch_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        base_environment_id: Uuid,
        name: String,
    ) -> Result<lapdev_common::kube::KubeEnvironment, ApiError> {
        let name = name.trim();
        if name.is_empty() {
            return Err(ApiError::InvalidRequest(
                "Environment name cannot be empty".to_string(),
            ));
        }
        let name = name.to_string();

        // Validate base environment
        let (base_environment, cluster, app_catalog) = self
            .validate_base_environment(org_id, base_environment_id)
            .await?;

        let branch_environment_id = Uuid::new_v4();
        let auth_token = rand_string(32);
        let namespace = base_environment.namespace.clone();

        // Create environment in database
        let mut created_env = match self
            .db
            .create_kube_environment(
                branch_environment_id,
                org_id,
                user_id,
                base_environment.app_catalog_id,
                base_environment.cluster_id,
                name.clone(),
                namespace.clone(),
                KubeEnvironmentStatus::Creating.to_string(),
                false, // Branch environments are always personal (not shared)
                base_environment.catalog_sync_version,
                Some(base_environment_id), // Set the base environment reference
                Vec::new(),
                HashMap::new(),
                auth_token.clone(),
            )
            .await
        {
            Ok(env) => env,
            Err(db_err) => {
                if let Some(sea_orm::SqlErr::UniqueConstraintViolation(constraint)) =
                    db_err.sql_err()
                {
                    if constraint == "kube_environment_cluster_namespace_unique_idx" {
                        return Err(ApiError::InternalError(
                            "Namespace allocation conflict. Please retry branch environment creation.".to_string(),
                        ));
                    }
                }
                return Err(ApiError::from(anyhow::Error::from(db_err)));
            }
        };

        // Notify kube-manager about the new branch environment
        self.notify_branch_environment_creation(
            base_environment_id,
            base_environment.cluster_id,
            &created_env,
        )
        .await;

        self.update_environment_status(created_env.id, KubeEnvironmentStatus::Running, None, None)
            .await?;
        created_env.status = KubeEnvironmentStatus::Running.to_string();

        // Build and return response
        Ok(Self::build_environment_response(
            created_env,
            app_catalog.name,
            cluster.name,
            Some(base_environment.name),
        ))
    }

    /// Authorize and validate that the environment can be synced from catalog
    async fn authorize_and_validate_sync(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<
        (
            lapdev_db_entities::kube_environment::Model,
            lapdev_db_entities::kube_app_catalog::Model,
        ),
        ApiError,
    > {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        let catalog = self
            .db
            .get_app_catalog(environment.app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        if catalog.sync_version == environment.catalog_sync_version {
            return Err(ApiError::InvalidRequest(
                "Environment is already up to date".to_string(),
            ));
        }

        Ok((environment, catalog))
    }

    /// Determine which workloads need to be synced by comparing sync versions
    async fn determine_workloads_to_sync(
        &self,
        environment_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<
        (
            Vec<lapdev_common::kube::KubeAppCatalogWorkload>,
            Vec<lapdev_common::kube::KubeAppCatalogWorkload>,
        ),
        ApiError,
    > {
        // Get existing environment workloads
        let existing_workloads = self
            .db
            .get_environment_workloads(environment_id)
            .await
            .map_err(ApiError::from)?;

        // Build a map of existing workloads by name for quick lookup
        let existing_workloads_map: std::collections::HashMap<String, _> = existing_workloads
            .into_iter()
            .map(|w| (w.name.clone(), w))
            .collect();

        let catalog_workloads = self
            .db
            .get_app_catalog_workloads(catalog_id)
            .await
            .map_err(ApiError::from)?;

        // Determine which workloads need to be deployed (added or updated)
        let workloads_to_deploy: Vec<_> = catalog_workloads
            .iter()
            .filter(|catalog_workload| {
                match existing_workloads_map.get(&catalog_workload.name) {
                    Some(existing_workload) => {
                        // Workload needs update if catalog version is newer
                        catalog_workload.catalog_sync_version
                            > existing_workload.catalog_sync_version
                    }
                    None => {
                        // New workload, needs to be deployed
                        true
                    }
                }
            })
            .cloned()
            .collect();

        Ok((catalog_workloads, workloads_to_deploy))
    }

    /// Deploy changed workloads to Kubernetes
    async fn perform_workload_deployment(
        &self,
        environment: &lapdev_db_entities::kube_environment::Model,
        catalog: &lapdev_db_entities::kube_app_catalog::Model,
        workloads_to_deploy: &[lapdev_common::kube::KubeAppCatalogWorkload],
        total_workloads: usize,
    ) -> Result<(HashSet<String>, Vec<NewEnvironmentWorkload>), ApiError> {
        if workloads_to_deploy.is_empty() {
            tracing::info!(
                "No workload changes detected for environment {} - skipping K8s deployment",
                environment.name
            );
            return Ok((HashSet::new(), Vec::new()));
        }

        tracing::info!(
            "Syncing {}/{} workloads for environment {} (only changed ones)",
            workloads_to_deploy.len(),
            total_workloads,
            environment.name
        );

        let target_server = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })?;

        let manager_namespace = self
            .db
            .get_kube_cluster(catalog.cluster_id)
            .await
            .map_err(ApiError::from)?
            .and_then(|cluster| cluster.manager_namespace);

        // Get workload YAML from database cache instead of querying Kubernetes
        // Handles workloads from multiple namespaces
        let (workloads_with_resources, workload_details) = self
            .get_catalog_workloads_with_yaml_from_db(
                catalog.cluster_id,
                workloads_to_deploy.to_vec(),
                &SidecarInjectionContext {
                    environment_id: environment.id,
                    namespace: &environment.namespace,
                    auth_token: &environment.auth_token,
                    manager_namespace: manager_namespace.as_deref(),
                },
            )
            .await?;

        let service_names: HashSet<String> =
            workloads_with_resources.services.keys().cloned().collect();

        self.deploy_environment_resources(
            &target_server,
            &environment,
            workloads_with_resources,
            None,
        )
        .await?;

        Ok((service_names, workload_details))
    }

    /// Update environment workloads in the database after successful deployment
    async fn update_environment_workloads_in_db(
        &self,
        environment_id: Uuid,
        environment_namespace: &str,
        catalog_workloads: &[lapdev_common::kube::KubeAppCatalogWorkload],
        workloads_to_deploy: &[NewEnvironmentWorkload],
        service_names: HashSet<String>,
        new_catalog_sync_version: i64,
    ) -> Result<(), ApiError> {
        let now = Utc::now().into();
        let txn = self.db.conn.begin().await.map_err(ApiError::from)?;

        // Build a set of catalog workload names for efficient lookup
        let catalog_workload_names: HashSet<String> =
            catalog_workloads.iter().map(|w| w.name.clone()).collect();

        // Only soft-delete workloads that no longer exist in the catalog
        let deleted_workloads = lapdev_db_entities::kube_environment_workload::Entity::find()
            .filter(
                lapdev_db_entities::kube_environment_workload::Column::EnvironmentId
                    .eq(environment_id),
            )
            .filter(lapdev_db_entities::kube_environment_workload::Column::DeletedAt.is_null())
            .all(&txn)
            .await
            .map_err(ApiError::from)?;

        for existing_workload in deleted_workloads {
            if !catalog_workload_names.contains(&existing_workload.name) {
                // This workload no longer exists in the catalog, soft-delete it
                lapdev_db_entities::kube_environment_workload::ActiveModel {
                    id: ActiveValue::Set(existing_workload.id),
                    deleted_at: ActiveValue::Set(Some(now)),
                    ..Default::default()
                }
                .update(&txn)
                .await
                .map_err(ApiError::from)?;
            }
        }

        // Soft-delete services that are no longer in use
        let mut service_delete_query =
            lapdev_db_entities::kube_environment_service::Entity::update_many()
                .filter(
                    lapdev_db_entities::kube_environment_service::Column::EnvironmentId
                        .eq(environment_id),
                )
                .filter(lapdev_db_entities::kube_environment_service::Column::DeletedAt.is_null());

        if !service_names.is_empty() {
            let existing: Vec<String> = service_names.into_iter().collect();
            service_delete_query = service_delete_query.filter(
                lapdev_db_entities::kube_environment_service::Column::Name.is_not_in(existing),
            );
        }

        service_delete_query
            .col_expr(
                lapdev_db_entities::kube_environment_service::Column::DeletedAt,
                Expr::value(now),
            )
            .exec(&txn)
            .await
            .map_err(ApiError::from)?;

        // Only insert/update workloads that were actually deployed
        for workload in workloads_to_deploy {
            let details = &workload.details;
            let containers_json = serde_json::to_value(&details.containers)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));
            let ports_json = serde_json::to_value(&details.ports)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));
            let workload_yaml = details.workload_yaml.clone();

            // First, soft-delete any existing workload with the same name
            lapdev_db_entities::kube_environment_workload::Entity::update_many()
                .filter(
                    lapdev_db_entities::kube_environment_workload::Column::EnvironmentId
                        .eq(environment_id),
                )
                .filter(
                    lapdev_db_entities::kube_environment_workload::Column::Name
                        .eq(details.name.clone()),
                )
                .filter(lapdev_db_entities::kube_environment_workload::Column::DeletedAt.is_null())
                .col_expr(
                    lapdev_db_entities::kube_environment_workload::Column::DeletedAt,
                    Expr::value(now),
                )
                .exec(&txn)
                .await
                .map_err(ApiError::from)?;

            // Then insert the new version
            lapdev_db_entities::kube_environment_workload::ActiveModel {
                id: ActiveValue::Set(workload.id),
                created_at: ActiveValue::Set(now),
                deleted_at: ActiveValue::Set(None),
                environment_id: ActiveValue::Set(environment_id),
                name: ActiveValue::Set(details.name.clone()),
                namespace: ActiveValue::Set(environment_namespace.to_string()),
                kind: ActiveValue::Set(details.kind.to_string()),
                containers: ActiveValue::Set(containers_json),
                ports: ActiveValue::Set(ports_json),
                workload_yaml: ActiveValue::Set(workload_yaml),
                catalog_sync_version: ActiveValue::Set(new_catalog_sync_version),
                base_workload_id: ActiveValue::Set(details.base_workload_id),
                ready_replicas: ActiveValue::Set(None),
            }
            .insert(&txn)
            .await
            .map_err(ApiError::from)?;
        }

        // Update environment's catalog sync version and timestamp
        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(environment_id),
            catalog_sync_version: ActiveValue::Set(new_catalog_sync_version),
            last_catalog_synced_at: ActiveValue::Set(Some(now)),
            sync_status: ActiveValue::Set(KubeEnvironmentSyncStatus::Idle.to_string()),
            ..Default::default()
        }
        .update(&txn)
        .await
        .map_err(ApiError::from)?;

        txn.commit().await.map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn sync_environment_from_catalog(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), ApiError> {
        // Authorize and validate that sync is needed
        let (environment, catalog) = match self
            .authorize_and_validate_sync(org_id, user_id, environment_id)
            .await
        {
            Ok(result) => result,
            Err(ApiError::InvalidRequest(msg)) if msg == "Environment is already up to date" => {
                return Ok(())
            }
            Err(e) => return Err(e),
        };

        // Mark environment as syncing
        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(environment.id),
            sync_status: ActiveValue::Set(KubeEnvironmentSyncStatus::Syncing.to_string()),
            ..Default::default()
        }
        .update(&self.db.conn)
        .await
        .map_err(ApiError::from)?;

        // Perform sync operations; reset status to idle on error
        let sync_result = async {
            // Determine which workloads need syncing
            let (catalog_workloads, workloads_to_deploy) = self
                .determine_workloads_to_sync(environment.id, catalog.id)
                .await?;

            // Deploy changed workloads to Kubernetes
            let (service_names, workload_details) = self
                .perform_workload_deployment(
                    &environment,
                    &catalog,
                    &workloads_to_deploy,
                    catalog_workloads.len(),
                )
                .await?;

            Ok::<_, ApiError>((catalog_workloads, workload_details, service_names))
        }
        .await;

        // Handle sync result - reset status on failure
        let (catalog_workloads, workload_details, service_names) = match sync_result {
            Ok(result) => result,
            Err(e) => {
                // Reset sync_status to idle on failure
                let _ = lapdev_db_entities::kube_environment::ActiveModel {
                    id: ActiveValue::Set(environment.id),
                    sync_status: ActiveValue::Set(KubeEnvironmentSyncStatus::Idle.to_string()),
                    ..Default::default()
                }
                .update(&self.db.conn)
                .await;
                return Err(e);
            }
        };

        // Update database with synced workloads
        self.update_environment_workloads_in_db(
            environment.id,
            &environment.namespace,
            &catalog_workloads,
            &workload_details,
            service_names,
            catalog.sync_version,
        )
        .await?;

        Ok(())
    }

    // Kube Namespace operations
    pub async fn create_kube_namespace(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        name: String,
        description: Option<String>,
        is_shared: bool,
    ) -> Result<lapdev_common::kube::KubeNamespace, ApiError> {
        let namespace = self
            .db
            .create_kube_namespace(org_id, user_id, name, description, is_shared)
            .await
            .map_err(ApiError::from)?;

        Ok(lapdev_common::kube::KubeNamespace {
            id: namespace.id,
            name: namespace.name,
            description: namespace.description,
            is_shared: namespace.is_shared,
            created_at: namespace.created_at,
            created_by: namespace.user_id,
        })
    }

    pub async fn get_all_kube_namespaces(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        is_shared: bool,
    ) -> Result<Vec<lapdev_common::kube::KubeNamespace>, ApiError> {
        let namespaces = self
            .db
            .get_all_kube_namespaces(org_id, user_id, is_shared)
            .await
            .map_err(ApiError::from)?;

        let kube_namespaces = namespaces
            .into_iter()
            .map(|ns| lapdev_common::kube::KubeNamespace {
                id: ns.id,
                name: ns.name,
                description: ns.description,
                is_shared: ns.is_shared,
                created_at: ns.created_at,
                created_by: ns.user_id,
            })
            .collect();

        Ok(kube_namespaces)
    }

    pub async fn delete_kube_namespace(
        &self,
        _org_id: Uuid,
        namespace_id: Uuid,
    ) -> Result<(), ApiError> {
        self.db
            .delete_kube_namespace(namespace_id)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    async fn publish_environment_event(&self, event: EnvironmentLifecycleEvent) {
        match serde_json::to_string(&event) {
            Ok(payload) => {
                if let Some(pool) = self.db.pool.clone() {
                    if let Err(err) = sqlx::query("SELECT pg_notify('environment_lifecycle', $1)")
                        .bind(payload)
                        .execute(&pool)
                        .await
                    {
                        warn!(
                            error = %err,
                            "failed to publish environment lifecycle event via NOTIFY"
                        );
                        let _ = self.environment_events.send(event);
                    }
                } else {
                    warn!("pg pool unavailable; broadcasting environment event locally");
                    let _ = self.environment_events.send(event);
                }
            }
            Err(err) => {
                warn!(
                    error = %err,
                    "failed to serialize environment lifecycle event"
                );
            }
        }
    }

    async fn update_environment_status(
        &self,
        environment_id: Uuid,
        status: KubeEnvironmentStatus,
        paused_at: Option<Option<chrono::DateTime<Utc>>>,
        resumed_at: Option<Option<chrono::DateTime<Utc>>>,
    ) -> Result<(), ApiError> {
        let paused_value = match paused_at {
            Some(Some(dt)) => ActiveValue::Set(Some(dt.into())),
            Some(None) => ActiveValue::Set(None),
            None => ActiveValue::NotSet,
        };

        let resumed_value = match resumed_at {
            Some(Some(dt)) => ActiveValue::Set(Some(dt.into())),
            Some(None) => ActiveValue::Set(None),
            None => ActiveValue::NotSet,
        };

        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(environment_id),
            status: ActiveValue::Set(status.to_string()),
            paused_at: paused_value,
            resumed_at: resumed_value,
            ..Default::default()
        }
        .update(&self.db.conn)
        .await
        .map_err(ApiError::from)?;

        if let Ok(Some(environment)) = self.db.get_kube_environment(environment_id).await {
            let event = EnvironmentLifecycleEvent {
                organization_id: environment.organization_id,
                environment_id,
                status: status.clone(),
                paused_at: environment.paused_at.map(|dt| dt.to_string()),
                resumed_at: environment.resumed_at.map(|dt| dt.to_string()),
                updated_at: Utc::now(),
            };

            self.publish_environment_event(event).await;
        }

        Ok(())
    }

    async fn run_pause_environment(&self, environment_id: Uuid) -> Result<(), ApiError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        let server = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })?;

        let mut workloads_with_resources =
            self.assemble_environment_workloads(&environment).await?;

        for workload in &mut workloads_with_resources.workloads {
            let result = match workload {
                KubeWorkloadYamlOnly::CronJob(_) => set_cronjob_suspend(workload, true),
                KubeWorkloadYamlOnly::Deployment(_)
                | KubeWorkloadYamlOnly::StatefulSet(_)
                | KubeWorkloadYamlOnly::ReplicaSet(_) => set_workload_replicas(workload, 0),
                KubeWorkloadYamlOnly::DaemonSet(_) => set_daemonset_paused(workload, true),
                KubeWorkloadYamlOnly::Pod(_) => {
                    warn!(
                        environment_id = %environment.id,
                        "Pod workloads cannot be gracefully paused; they will continue running"
                    );
                    Ok(())
                }
                _ => Ok(()),
            };
            if let Err(err) = result {
                warn!(
                    environment_id = %environment.id,
                    error = ?err,
                    "Failed to update workload YAML while pausing environment"
                );
            }
        }

        self.deploy_environment_resources(&server, &environment, workloads_with_resources, None)
            .await?;

        if let Some(base_env_id) = environment.base_environment_id {
            if let Err(err) = self
                .notify_branch_environment_deletion(&server.rpc_client, base_env_id, environment.id)
                .await
            {
                warn!(
                    environment_id = %environment.id,
                    error = ?err,
                    "Failed to notify devbox proxy about branch environment pause"
                );
            }
        }

        self.update_environment_status(
            environment.id,
            KubeEnvironmentStatus::Paused,
            Some(Some(Utc::now())),
            Some(None),
        )
        .await
    }

    async fn run_resume_environment(&self, environment_id: Uuid) -> Result<(), ApiError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        let server = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })?;

        let mut workloads_with_resources =
            self.assemble_environment_workloads(&environment).await?;

        for workload in &mut workloads_with_resources.workloads {
            let result = match workload {
                KubeWorkloadYamlOnly::CronJob(_) => set_cronjob_suspend(workload, false),
                KubeWorkloadYamlOnly::Deployment(_)
                | KubeWorkloadYamlOnly::StatefulSet(_)
                | KubeWorkloadYamlOnly::ReplicaSet(_) => set_workload_replicas(workload, 1),
                KubeWorkloadYamlOnly::DaemonSet(_) => set_daemonset_paused(workload, false),
                KubeWorkloadYamlOnly::Pod(_) => Ok(()),
                _ => Ok(()),
            };
            if let Err(err) = result {
                warn!(
                    environment_id = %environment.id,
                    error = ?err,
                    "Failed to update workload YAML while resuming environment"
                );
            }
        }

        self.deploy_environment_resources(&server, &environment, workloads_with_resources, None)
            .await?;

        if let Some(base_env_id) = environment.base_environment_id {
            self.notify_branch_environment_creation(
                base_env_id,
                environment.cluster_id,
                &environment,
            )
            .await;
        }

        self.update_environment_status(
            environment.id,
            KubeEnvironmentStatus::Running,
            None,
            Some(Some(Utc::now())),
        )
        .await
    }
}
