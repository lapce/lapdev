use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

use anyhow::Result;
use lapdev_common::{
    kube::{
        CreateKubeClusterResponse, KubeAppCatalog, KubeCluster, KubeClusterInfo, KubeClusterStatus,
        KubeEnvironment, KubeNamespace, KubeWorkload, KubeWorkloadKind, KubeWorkloadList,
        PagePaginationParams, PaginatedInfo, PaginatedResult, PaginationParams,
    },
    token::PlainToken,
};
use lapdev_db::api::DbApi;
use lapdev_kube::server::KubeClusterServer;
use lapdev_rpc::error::ApiError;
use secrecy::ExposeSecret;

#[derive(Clone)]
pub struct KubeController {
    // KubeManager connections per cluster
    pub kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
    // Database API
    pub db: DbApi,
}

impl KubeController {
    pub fn new(db: DbApi) -> Self {
        Self {
            kube_cluster_servers: Arc::new(RwLock::new(HashMap::new())),
            db,
        }
    }

    pub async fn get_random_kube_cluster_server(
        &self,
        cluster_id: Uuid,
    ) -> Option<KubeClusterServer> {
        let servers = self.kube_cluster_servers.read().await;
        servers.get(&cluster_id)?.last().cloned()
    }

    pub async fn get_all_kube_clusters(&self, org_id: Uuid) -> Result<Vec<KubeCluster>, ApiError> {
        let clusters = self
            .db
            .get_all_kube_clusters(org_id)
            .await
            .map_err(ApiError::from)?
            .into_iter()
            .map(|c| KubeCluster {
                id: c.id,
                name: c.name.clone(),
                can_deploy: c.can_deploy,
                info: KubeClusterInfo {
                    cluster_name: Some(c.name),
                    cluster_version: c.cluster_version.unwrap_or("Unknown".to_string()),
                    node_count: 0, // TODO: Get actual node count from kube-manager
                    available_cpu: "N/A".to_string(), // TODO: Get actual CPU from kube-manager
                    available_memory: "N/A".to_string(), // TODO: Get actual memory from kube-manager
                    provider: None,                      // TODO: Get provider info
                    region: c.region,
                    status: c
                        .status
                        .as_deref()
                        .and_then(|s| KubeClusterStatus::from_str(s).ok())
                        .unwrap_or(KubeClusterStatus::NotReady),
                },
            })
            .collect();
        Ok(clusters)
    }

    pub async fn create_kube_cluster(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, ApiError> {
        // Generate cluster ID and token
        let cluster_id = Uuid::new_v4();
        let token = PlainToken::generate();
        let hashed_token = token.hashed();
        let token_name = format!("{name}-default");

        // Create the cluster
        self.db
            .create_kube_cluster(
                cluster_id,
                org_id,
                user_id,
                name,
                Some(KubeClusterStatus::Provisioning.to_string()),
            )
            .await
            .map_err(ApiError::from)?;

        // Create the cluster token
        self.db
            .create_kube_cluster_token(
                cluster_id,
                user_id,
                token_name,
                hashed_token.expose_secret().to_vec(),
            )
            .await
            .map_err(ApiError::from)?;

        Ok(CreateKubeClusterResponse {
            cluster_id,
            token: token.expose_secret().to_string(),
        })
    }

    pub async fn delete_kube_cluster(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<(), ApiError> {
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

        // Check for dependencies - kube_app_catalog
        let has_app_catalogs = self
            .db
            .check_kube_cluster_has_app_catalogs(cluster_id)
            .await
            .map_err(ApiError::from)?;

        if has_app_catalogs {
            return Err(ApiError::InvalidRequest(
                "Cannot delete cluster: it has active app catalogs. Please delete them first.".to_string()
            ));
        }

        // Check for dependencies - kube_environment
        let has_environments = self
            .db
            .check_kube_cluster_has_environments(cluster_id)
            .await
            .map_err(ApiError::from)?;

        if has_environments {
            return Err(ApiError::InvalidRequest(
                "Cannot delete cluster: it has active environments. Please delete them first.".to_string()
            ));
        }

        // Soft delete the cluster
        self.db
            .delete_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn set_cluster_deployable(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), ApiError> {
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

        // Update the can_deploy field
        self.db
            .set_cluster_deployable(cluster_id, can_deploy)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn get_workloads(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, ApiError> {
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

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        let pagination = pagination.unwrap_or(PaginationParams {
            cursor: None,
            limit: 20,
        });

        // Call KubeManager to get workloads
        match server
            .rpc_client
            .get_workloads(
                tarpc::context::current(),
                namespace,
                workload_kind_filter,
                include_system_workloads,
                Some(pagination),
            )
            .await
        {
            Ok(Ok(workload_list)) => Ok(workload_list),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!(
                "KubeManager error: {}",
                e
            ))),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e))),
        }
    }

    pub async fn get_workload_details(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, ApiError> {
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

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Call KubeManager to get workload details
        match server
            .rpc_client
            .get_workload_details(tarpc::context::current(), name, namespace)
            .await
        {
            Ok(Ok(workload_details)) => Ok(workload_details),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!(
                "KubeManager error: {}",
                e
            ))),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e))),
        }
    }

    pub async fn get_namespaces(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespace>, ApiError> {
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

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Call KubeManager to get namespaces
        match server
            .rpc_client
            .get_namespaces(tarpc::context::current())
            .await
        {
            Ok(Ok(namespaces)) => Ok(namespaces),
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!(
                "KubeManager error: {}",
                e
            ))),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e))),
        }
    }

    pub async fn get_cluster_info(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeClusterInfo, ApiError> {
        // Get cluster from database
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Convert database cluster to KubeClusterInfo
        let cluster_info = KubeClusterInfo {
            cluster_name: Some(cluster.name),
            cluster_version: cluster.cluster_version.unwrap_or("Unknown".to_string()),
            node_count: 0, // TODO: Get actual node count from kube-manager
            available_cpu: "N/A".to_string(), // TODO: Get actual CPU from kube-manager
            available_memory: "N/A".to_string(), // TODO: Get actual memory from kube-manager
            provider: None, // TODO: Get provider info
            region: cluster.region,
            status: cluster
                .status
                .as_deref()
                .and_then(|s| KubeClusterStatus::from_str(s).ok())
                .unwrap_or(KubeClusterStatus::NotReady),
        };

        Ok(cluster_info)
    }

    pub async fn create_app_catalog(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        resources: String,
    ) -> Result<(), ApiError> {
        self.db
            .create_app_catalog(org_id, user_id, cluster_id, name, description, resources)
            .await
            .map_err(ApiError::from)
    }

    pub async fn get_all_app_catalogs(
        &self,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeAppCatalog>, ApiError> {
        let pagination = pagination.unwrap_or_default();
        
        let (catalogs_with_clusters, total_count) = self
            .db
            .get_all_app_catalogs_paginated(org_id, search, Some(pagination.clone()))
            .await
            .map_err(ApiError::from)?;

        let app_catalogs = catalogs_with_clusters
            .into_iter()
            .filter_map(|(catalog, cluster)| {
                let cluster = cluster?;
                Some(KubeAppCatalog {
                    id: catalog.id,
                    name: catalog.name,
                    description: catalog.description,
                    created_at: catalog.created_at,
                    created_by: catalog.created_by,
                    cluster_id: catalog.cluster_id,
                    cluster_name: cluster.name,
                })
            })
            .collect();

        let total_pages = (total_count + pagination.page_size - 1) / pagination.page_size;

        Ok(PaginatedResult {
            data: app_catalogs,
            pagination_info: PaginatedInfo {
                total_count,
                page: pagination.page,
                page_size: pagination.page_size,
                total_pages,
            },
        })
    }

    pub async fn get_all_kube_environments(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, ApiError> {
        let pagination = pagination.unwrap_or_default();
        
        let (environments_with_catalogs, total_count) = self
            .db
            .get_all_kube_environments_paginated(org_id, user_id, search, Some(pagination.clone()))
            .await
            .map_err(ApiError::from)?;

        let kube_environments = environments_with_catalogs
            .into_iter()
            .filter_map(|(env, catalog)| {
                let catalog = catalog?;
                Some(KubeEnvironment {
                    id: env.id,
                    name: env.name,
                    namespace: env.namespace,
                    app_catalog_id: env.app_catalog_id,
                    app_catalog_name: catalog.name,
                    status: env.status,
                    created_at: env.created_at.to_string(),
                    created_by: env.created_by,
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

    pub async fn delete_app_catalog(&self, org_id: Uuid, catalog_id: Uuid) -> Result<(), ApiError> {
        // Verify catalog belongs to the organization
        let catalog = self
            .db
            .get_app_catalog(catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Check for dependencies - kube_environment (any user's environments using this catalog)
        let has_environments = self
            .db
            .check_app_catalog_has_environments(catalog_id)
            .await
            .map_err(ApiError::from)?;

        if has_environments {
            return Err(ApiError::InvalidRequest(
                "Cannot delete app catalog: it has active environments. Please delete them first.".to_string()
            ));
        }

        // Soft delete the app catalog
        self.db
            .delete_app_catalog(catalog_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn create_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<(), ApiError> {
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

        // Check if the cluster allows deployments
        if !cluster.can_deploy {
            return Err(ApiError::InvalidRequest(
                "Deployments are not allowed on this cluster".to_string()
            ));
        }

        // Create the environment in database
        self.db
            .create_kube_environment(
                org_id,
                user_id,
                app_catalog_id,
                cluster_id,
                name.clone(),
                namespace.clone(),
                Some("Pending".to_string()),
            )
            .await
            .map_err(ApiError::from)?;

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the environment target cluster".to_string(),
                )
            })?;

        // Deploy the app catalog resources to the cluster
        self.deploy_app_catalog(&server, &namespace, &name, &app_catalog)
            .await?;

        Ok(())
    }

    async fn deploy_app_catalog(
        &self,
        target_server: &KubeClusterServer,
        namespace: &str,
        environment_name: &str,
        app_catalog: &lapdev_db_entities::kube_app_catalog::Model,
    ) -> Result<(), ApiError> {
        tracing::info!(
            "Deploying app catalog resources for environment '{}' in namespace '{}'",
            environment_name,
            namespace
        );

        // Step 1: Parse the JSON resources into KubeWorkload structures
        let workloads: Vec<KubeWorkload> =
            serde_json::from_str(&app_catalog.resources).map_err(|e| {
                ApiError::InternalError(format!("Failed to parse app catalog resources: {e}"))
            })?;

        if workloads.is_empty() {
            tracing::warn!(
                "No workloads found in app catalog for environment '{}'",
                environment_name
            );
            return Ok(());
        }

        tracing::info!(
            "Found {} workloads to deploy for environment '{}'",
            workloads.len(),
            environment_name
        );

        // Step 2: Prepare environment-specific labels
        let mut environment_labels = std::collections::HashMap::new();
        environment_labels.insert(
            "lapdev.environment".to_string(),
            environment_name.to_string(),
        );
        environment_labels.insert("lapdev.managed-by".to_string(), "lapdev".to_string());

        // Step 3: Get the source cluster server where the app catalog was created
        let source_server = self
            .get_random_kube_cluster_server(app_catalog.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the app catalog's source cluster".to_string(),
                )
            })?;

        // Step 4: Deploy each workload (retrieve YAML from source, deploy to target)
        for workload in workloads {
            // First, retrieve the original YAML manifest from the source cluster
            let workload_yaml = match source_server
                .rpc_client
                .get_workload_yaml(
                    tarpc::context::current(),
                    workload.name.clone(),
                    workload.namespace.clone(),
                    workload.kind.clone(),
                )
                .await
            {
                Ok(Ok(workload_yaml)) => workload_yaml,
                Ok(Err(e)) => {
                    tracing::error!(
                        "Failed to get YAML for workload '{}' from source cluster: {}",
                        workload.name,
                        e
                    );
                    return Err(ApiError::InvalidRequest(format!(
                        "Failed to get YAML for workload '{}' from source cluster: {}",
                        workload.name, e
                    )));
                }
                Err(e) => {
                    return Err(ApiError::InvalidRequest(format!(
                        "Connection error to source cluster: {}",
                        e
                    )));
                }
            };

            // Then deploy the workload to the target cluster
            match target_server
                .rpc_client
                .deploy_workload_yaml(
                    tarpc::context::current(),
                    namespace.to_string(),
                    workload_yaml,
                    environment_labels.clone(),
                )
                .await
            {
                Ok(Ok(())) => {
                    tracing::info!(
                        "Successfully deployed workload '{}' of kind '{:?}' to target cluster",
                        workload.name,
                        workload.kind
                    );
                }
                Ok(Err(e)) => {
                    tracing::error!(
                        "Failed to deploy workload '{}' to target cluster: {}",
                        workload.name,
                        e
                    );
                    return Err(ApiError::InvalidRequest(format!(
                        "Failed to deploy workload '{}' to target cluster: {}",
                        workload.name, e
                    )));
                }
                Err(e) => {
                    return Err(ApiError::InvalidRequest(format!(
                        "Connection error to target cluster: {}",
                        e
                    )));
                }
            }
        }

        tracing::info!(
            "Successfully deployed all resources for environment '{}'",
            environment_name
        );
        Ok(())
    }
}
