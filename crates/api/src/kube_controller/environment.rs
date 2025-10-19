use chrono::Utc;
use lapdev_common::{
    kube::{
        KubeContainerImage, KubeEnvironment, KubeEnvironmentSyncStatus, KubeWorkloadDetails,
        PagePaginationParams, PaginatedInfo, PaginatedResult,
    },
    utils::rand_string,
};
use lapdev_rpc::error::ApiError;
use sea_orm::{
    prelude::Json, sea_query::Expr, ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait,
    QueryFilter, TransactionTrait,
};
use std::str::FromStr;
use uuid::Uuid;

use super::{EnvironmentNamespaceKind, KubeController};

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
                let last_catalog_synced_at = env.last_catalog_synced_at.map(|dt| dt.to_string());
                let catalog_update_available = catalog.sync_version > catalog_sync_version;

                Some(KubeEnvironment {
                    id: env.id,
                    name: env.name,
                    namespace: env.namespace,
                    app_catalog_id: env.app_catalog_id,
                    app_catalog_name: catalog.name,
                    cluster_id: env.cluster_id,
                    cluster_name: cluster.name,
                    status: env.status,
                    created_at: env.created_at.to_string(),
                    user_id: env.user_id,
                    is_shared: env.is_shared,
                    base_environment_id: env.base_environment_id,
                    base_environment_name,
                    catalog_sync_version,
                    last_catalog_synced_at,
                    catalog_update_available,
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

        Ok(KubeEnvironment {
            id: environment.id,
            user_id: environment.user_id,
            name: environment.name,
            namespace: environment.namespace,
            app_catalog_id: environment.app_catalog_id,
            app_catalog_name: catalog.name,
            cluster_id: environment.cluster_id,
            cluster_name: cluster.name,
            status: environment.status,
            created_at: environment
                .created_at
                .format("%Y-%m-%d %H:%M:%S%.f %z")
                .to_string(),
            is_shared: environment.is_shared,
            base_environment_id: environment.base_environment_id,
            base_environment_name,
            catalog_sync_version: environment.catalog_sync_version,
            last_catalog_synced_at: environment.last_catalog_synced_at.map(|dt| dt.to_string()),
            catalog_update_available,
            sync_status: KubeEnvironmentSyncStatus::from_str(&environment.sync_status)
                .unwrap_or(KubeEnvironmentSyncStatus::Idle),
        })
    }

    pub async fn delete_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), ApiError> {
        // First get the environment to check ownership
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

        // If this is a branch environment, notify the base environment's devbox-proxy before deletion
        if let Some(base_env_id) = environment.base_environment_id {
            match rpc_client
                .remove_branch_environment(tarpc::context::current(), base_env_id, environment_id)
                .await
            {
                Ok(Ok(())) => {
                    tracing::info!(
                        "Successfully notified devbox-proxy about branch environment {} deletion",
                        environment_id
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        "Failed to notify devbox-proxy about branch environment {} deletion: {}",
                        environment_id,
                        e
                    );
                    return Err(ApiError::InvalidRequest(format!(
                        "Failed to notify devbox-proxy about branch environment deletion: {e}"
                    )));
                }
                Err(e) => {
                    tracing::error!(
                        "RPC call failed when notifying about branch environment {} deletion: {}",
                        environment_id,
                        e
                    );
                    return Err(ApiError::InvalidRequest(format!(
                        "Connection error while notifying devbox-proxy: {e}"
                    )));
                }
            }
        }

        match rpc_client
            .destroy_environment(
                tarpc::context::current(),
                environment.id,
                environment.namespace.clone(),
            )
            .await
        {
            Ok(Ok(())) => {
                tracing::info!(
                    "Successfully deleted resources for environment {} in namespace {}",
                    environment.id,
                    environment.namespace
                );
            }
            Ok(Err(e)) => {
                tracing::error!(
                    "KubeManager error when deleting environment {}: {}",
                    environment.id,
                    e
                );
                return Err(ApiError::InvalidRequest(format!(
                    "Failed to delete environment resources: {e}"
                )));
            }
            Err(e) => {
                tracing::error!(
                    "Connection error when deleting environment {}: {}",
                    environment.id,
                    e
                );
                return Err(ApiError::InvalidRequest(format!(
                    "Failed to communicate with KubeManager to delete environment: {e}"
                )));
            }
        }

        // Delete from database (soft delete)
        self.db
            .delete_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?;

        Ok(())
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
        let name = name.trim();
        if name.is_empty() {
            return Err(ApiError::InvalidRequest(
                "Environment name cannot be empty".to_string(),
            ));
        }
        let name = name.to_string();

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

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })?;

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

        // First, get workloads YAML before creating in database to validate success
        let workloads_with_resources = self
            .get_workloads_yaml_for_catalog(&app_catalog, workloads.clone())
            .await?;

        let namespace = self
            .generate_unique_namespace(
                cluster_id,
                if is_shared {
                    EnvironmentNamespaceKind::Shared
                } else {
                    EnvironmentNamespaceKind::Personal
                },
            )
            .await?;

        let services_map = workloads_with_resources.services.clone();

        // Store the environment workloads in the database before deployment
        let workload_details: Vec<KubeWorkloadDetails> = workloads
            .into_iter()
            .map(|workload| {
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
                lapdev_common::kube::KubeWorkloadDetails {
                    name: workload.name,
                    namespace: namespace.clone(),
                    kind: workload.kind,
                    containers,
                    ports: workload.ports,
                    workload_yaml: workload.workload_yaml.unwrap_or_default(),
                }
            })
            .collect();

        let created_env = match self
            .db
            .create_kube_environment(
                org_id,
                user_id,
                app_catalog_id,
                cluster_id,
                name.clone(),
                namespace.clone(),
                "Pending".to_string(),
                is_shared,
                app_catalog.sync_version,
                None, // No base environment for regular environments
                workload_details,
                services_map,
            )
            .await
        {
            Ok(env) => env,
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
                return Err(ApiError::from(anyhow::Error::from(db_err)));
            }
        };

        // Deploy the app catalog resources to the cluster using pre-fetched YAML
        self.deploy_app_catalog_with_yaml(
            &server,
            &created_env.namespace,
            &name,
            created_env.id,
            Some(created_env.auth_token.clone()),
            workloads_with_resources,
        )
        .await?;

        // Convert the database model to the API type
        Ok(lapdev_common::kube::KubeEnvironment {
            id: created_env.id,
            user_id: created_env.user_id,
            name: created_env.name,
            namespace: created_env.namespace,
            status: created_env.status,
            is_shared: created_env.is_shared,
            app_catalog_id: created_env.app_catalog_id,
            app_catalog_name: app_catalog.name,
            cluster_id: created_env.cluster_id,
            cluster_name: cluster.name,
            created_at: created_env.created_at.to_string(),
            base_environment_id: created_env.base_environment_id,
            base_environment_name: None, // Regular environments have no base environment
            catalog_sync_version: created_env.catalog_sync_version,
            last_catalog_synced_at: created_env.last_catalog_synced_at.map(|dt| dt.to_string()),
            catalog_update_available: false,
            sync_status: KubeEnvironmentSyncStatus::from_str(&created_env.sync_status)
                .unwrap_or(KubeEnvironmentSyncStatus::Idle),
        })
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

        // Get the cluster to verify personal deployments are allowed
        let cluster = self
            .db
            .get_kube_cluster(base_environment.cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        // Get workloads and services from the base environment
        let base_workloads = self
            .db
            .get_environment_workloads(base_environment_id)
            .await
            .map_err(ApiError::from)?;

        let base_services = self
            .db
            .get_environment_services(base_environment_id)
            .await
            .map_err(ApiError::from)?;

        let namespace = self
            .generate_unique_namespace(
                base_environment.cluster_id,
                EnvironmentNamespaceKind::Branch,
            )
            .await?;

        let services_map: std::collections::HashMap<
            String,
            lapdev_common::kube::KubeServiceWithYaml,
        > = base_services
            .into_iter()
            .map(|service| {
                (
                    service.name.clone(),
                    lapdev_common::kube::KubeServiceWithYaml {
                        yaml: service.yaml,
                        details: lapdev_common::kube::KubeServiceDetails {
                            name: service.name,
                            ports: service.ports,
                            selector: service.selector,
                        },
                    },
                )
            })
            .collect();

        // Get app catalog info for the response
        let app_catalog = self
            .db
            .get_app_catalog(base_environment.app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        // Convert workloads to the format needed for database creation
        let workload_details: Vec<KubeWorkloadDetails> = base_workloads
            .into_iter()
            .filter_map(|workload| {
                workload.kind.parse().ok().map(|kind| {
                    let mut containers = workload.containers;
                    for container in &mut containers {
                        // Preserve the original environment variables
                        container.original_env_vars = container.env_vars.clone();
                        container.env_vars.clear();

                        // If the base environment has a customized image, use it as the original_image
                        // for the new branch environment (so the branch starts from the customized state)
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
                    lapdev_common::kube::KubeWorkloadDetails {
                        name: workload.name,
                        namespace: namespace.clone(),
                        kind,
                        containers,
                        ports: workload.ports,
                        workload_yaml: String::new(),
                    }
                })
            })
            .collect();

        let created_env = match self
            .db
            .create_kube_environment(
                org_id,
                user_id,
                base_environment.app_catalog_id,
                base_environment.cluster_id,
                name.clone(),
                namespace.clone(),
                "Pending".to_string(),
                false, // Branch environments are always personal (not shared)
                base_environment.catalog_sync_version,
                Some(base_environment_id), // Set the base environment reference
                workload_details,
                services_map,
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

        // Notify kube-manager about the new branch environment via RPC
        if let Some(server) = self
            .get_random_kube_cluster_server(base_environment.cluster_id)
            .await
        {
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
                base_environment.cluster_id,
                created_env.id
            );
        }

        // Convert the database model to the API type
        Ok(lapdev_common::kube::KubeEnvironment {
            id: created_env.id,
            user_id: created_env.user_id,
            name: created_env.name,
            namespace: created_env.namespace,
            status: created_env.status,
            is_shared: created_env.is_shared,
            app_catalog_id: created_env.app_catalog_id,
            app_catalog_name: app_catalog.name,
            cluster_id: created_env.cluster_id,
            cluster_name: cluster.name,
            created_at: created_env.created_at.to_string(),
            base_environment_id: created_env.base_environment_id,
            base_environment_name: Some(base_environment.name), // Branch environments have the base environment name
            catalog_sync_version: created_env.catalog_sync_version,
            last_catalog_synced_at: created_env.last_catalog_synced_at.map(|dt| dt.to_string()),
            catalog_update_available: false,
            sync_status: KubeEnvironmentSyncStatus::from_str(&created_env.sync_status)
                .unwrap_or(KubeEnvironmentSyncStatus::Idle),
        })
    }

    pub async fn sync_environment_from_catalog(
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
            // Already up to date; nothing to do
            return Ok(());
        }

        // Set sync_status to "syncing" before starting
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
            let target_server = self
                .get_random_kube_cluster_server(environment.cluster_id)
                .await
                .ok_or_else(|| {
                    ApiError::InvalidRequest(
                        "No connected KubeManager for the environment target cluster".to_string(),
                    )
                })?;

            let catalog_workloads = self
                .db
                .get_app_catalog_workloads(catalog.id)
                .await
                .map_err(ApiError::from)?;

            let workloads_with_resources = self
                .get_workloads_yaml_for_catalog(&catalog, catalog_workloads.clone())
                .await?;

            self.deploy_app_catalog_with_yaml(
                &target_server,
                &environment.namespace,
                &environment.name,
                environment.id,
                Some(environment.auth_token.clone()),
                workloads_with_resources,
            )
            .await?;

            Ok::<_, ApiError>(catalog_workloads)
        }
        .await;

        // If sync failed, reset status to idle and return error
        let catalog_workloads = match sync_result {
            Ok(workloads) => workloads,
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

        let now = Utc::now().into();
        let txn = self.db.conn.begin().await.map_err(ApiError::from)?;

        lapdev_db_entities::kube_environment_workload::Entity::update_many()
            .filter(
                lapdev_db_entities::kube_environment_workload::Column::EnvironmentId
                    .eq(environment.id),
            )
            .filter(lapdev_db_entities::kube_environment_workload::Column::DeletedAt.is_null())
            .col_expr(
                lapdev_db_entities::kube_environment_workload::Column::DeletedAt,
                Expr::value(now),
            )
            .exec(&txn)
            .await
            .map_err(ApiError::from)?;

        for workload in &catalog_workloads {
            let containers_json = serde_json::to_value(&workload.containers)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));
            let ports_json = serde_json::to_value(&workload.ports)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            lapdev_db_entities::kube_environment_workload::ActiveModel {
                id: ActiveValue::Set(Uuid::new_v4()),
                created_at: ActiveValue::Set(now),
                deleted_at: ActiveValue::Set(None),
                environment_id: ActiveValue::Set(environment.id),
                name: ActiveValue::Set(workload.name.clone()),
                namespace: ActiveValue::Set(environment.namespace.clone()),
                kind: ActiveValue::Set(workload.kind.to_string()),
                containers: ActiveValue::Set(containers_json),
                ports: ActiveValue::Set(ports_json),
                catalog_sync_version: ActiveValue::Set(catalog.sync_version),
            }
            .insert(&txn)
            .await
            .map_err(ApiError::from)?;
        }

        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(environment.id),
            catalog_sync_version: ActiveValue::Set(catalog.sync_version),
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
}
