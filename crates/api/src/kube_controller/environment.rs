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
use std::{collections::HashSet, str::FromStr};
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

    /// Prepare workload details for environment creation by copying from catalog workloads
    fn prepare_workload_details_from_catalog(
        workloads: Vec<lapdev_common::kube::KubeAppCatalogWorkload>,
        namespace: &str,
    ) -> Vec<KubeWorkloadDetails> {
        workloads
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
                    namespace: namespace.to_string(),
                    kind: workload.kind,
                    containers,
                    ports: workload.ports,
                    workload_yaml: workload.workload_yaml.unwrap_or_default(),
                }
            })
            .collect()
    }

    /// Build KubeEnvironment response from database model
    fn build_environment_response(
        created_env: lapdev_db_entities::kube_environment::Model,
        app_catalog_name: String,
        cluster_name: String,
        base_environment_name: Option<String>,
    ) -> lapdev_common::kube::KubeEnvironment {
        lapdev_common::kube::KubeEnvironment {
            id: created_env.id,
            user_id: created_env.user_id,
            name: created_env.name,
            namespace: created_env.namespace,
            status: created_env.status,
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
            catalog_update_available: false,
            sync_status: KubeEnvironmentSyncStatus::from_str(&created_env.sync_status)
                .unwrap_or(KubeEnvironmentSyncStatus::Idle),
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

        // Destroy environment resources in Kubernetes
        self.destroy_environment_resources(&rpc_client, &environment)
            .await?;

        // Delete from database (soft delete)
        self.db
            .delete_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?;

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
        let name = name.trim();
        if name.is_empty() {
            return Err(ApiError::InvalidRequest(
                "Environment name cannot be empty".to_string(),
            ));
        }
        let name = name.to_string();

        // Validate catalog and cluster
        let (app_catalog, cluster) = self
            .validate_environment_creation(org_id, app_catalog_id, cluster_id, is_shared)
            .await?;

        // Get a connected KubeClusterServer for deployment
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })?;

        // Get workloads from catalog
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

        // Get workloads YAML to validate before creating in database
        let workloads_with_resources = self
            .get_catalog_workloads_with_yaml_from_db(app_catalog.cluster_id, workloads.clone())
            .await?;

        // Generate unique namespace
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

        // Prepare workload details for database
        let workload_details = Self::prepare_workload_details_from_catalog(workloads, &namespace);

        // Create environment in database
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

        // Deploy resources to Kubernetes
        self.deploy_environment_resources(
            &server,
            &created_env.namespace,
            &name,
            created_env.id,
            Some(created_env.auth_token.clone()),
            workloads_with_resources,
        )
        .await?;

        // Build and return response
        Ok(Self::build_environment_response(
            created_env,
            app_catalog.name,
            cluster.name,
            None,
        ))
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

    /// Prepare workload details from base environment workloads
    fn prepare_workload_details_from_base(
        base_workloads: Vec<lapdev_common::kube::KubeEnvironmentWorkload>,
        namespace: &str,
    ) -> Vec<KubeWorkloadDetails> {
        base_workloads
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
                        namespace: namespace.to_string(),
                        kind,
                        containers,
                        ports: workload.ports,
                        workload_yaml: String::new(),
                    }
                })
            })
            .collect()
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

        // Generate unique namespace
        let namespace = self
            .generate_unique_namespace(
                base_environment.cluster_id,
                EnvironmentNamespaceKind::Branch,
            )
            .await?;

        // Prepare services map
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

        // Prepare workload details
        let workload_details = Self::prepare_workload_details_from_base(base_workloads, &namespace);

        // Create environment in database
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

        // Notify kube-manager about the new branch environment
        self.notify_branch_environment_creation(
            base_environment_id,
            base_environment.cluster_id,
            &created_env,
        )
        .await;

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
    ) -> Result<HashSet<String>, ApiError> {
        if workloads_to_deploy.is_empty() {
            tracing::info!(
                "No workload changes detected for environment {} - skipping K8s deployment",
                environment.name
            );
            return Ok(HashSet::new());
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

        // Get workload YAML from database cache instead of querying Kubernetes
        // Handles workloads from multiple namespaces
        let workloads_with_resources = self
            .get_catalog_workloads_with_yaml_from_db(
                catalog.cluster_id,
                workloads_to_deploy.to_vec(),
            )
            .await?;

        let service_names: HashSet<String> =
            workloads_with_resources.services.keys().cloned().collect();

        self.deploy_environment_resources(
            &target_server,
            &environment.namespace,
            &environment.name,
            environment.id,
            Some(environment.auth_token.clone()),
            workloads_with_resources,
        )
        .await?;

        Ok(service_names)
    }

    /// Update environment workloads in the database after successful deployment
    async fn update_environment_workloads_in_db(
        &self,
        environment_id: Uuid,
        environment_namespace: &str,
        catalog_workloads: &[lapdev_common::kube::KubeAppCatalogWorkload],
        workloads_to_deploy: &[lapdev_common::kube::KubeAppCatalogWorkload],
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
            let containers_json = serde_json::to_value(&workload.containers)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));
            let ports_json = serde_json::to_value(&workload.ports)
                .map(Json::from)
                .unwrap_or_else(|_| Json::from(serde_json::json!([])));

            // First, soft-delete any existing workload with the same name
            lapdev_db_entities::kube_environment_workload::Entity::update_many()
                .filter(
                    lapdev_db_entities::kube_environment_workload::Column::EnvironmentId
                        .eq(environment_id),
                )
                .filter(
                    lapdev_db_entities::kube_environment_workload::Column::Name
                        .eq(workload.name.clone()),
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
                id: ActiveValue::Set(Uuid::new_v4()),
                created_at: ActiveValue::Set(now),
                deleted_at: ActiveValue::Set(None),
                environment_id: ActiveValue::Set(environment_id),
                name: ActiveValue::Set(workload.name.clone()),
                namespace: ActiveValue::Set(environment_namespace.to_string()),
                kind: ActiveValue::Set(workload.kind.to_string()),
                containers: ActiveValue::Set(containers_json),
                ports: ActiveValue::Set(ports_json),
                catalog_sync_version: ActiveValue::Set(new_catalog_sync_version),
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
            let service_names = self
                .perform_workload_deployment(
                    &environment,
                    &catalog,
                    &workloads_to_deploy,
                    catalog_workloads.len(),
                )
                .await?;

            Ok::<_, ApiError>((catalog_workloads, workloads_to_deploy, service_names))
        }
        .await;

        // Handle sync result - reset status on failure
        let (catalog_workloads, workloads_to_deploy, service_names) = match sync_result {
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
            &workloads_to_deploy,
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
}
