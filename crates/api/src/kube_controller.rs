use std::{collections::HashMap, str::FromStr, sync::Arc};
use tokio::sync::RwLock;
use uuid::Uuid;

use anyhow::Result;
use lapdev_common::{
    kube::{
        CreateKubeClusterResponse, KubeAppCatalog, KubeAppCatalogWorkload,
        KubeAppCatalogWorkloadCreate, KubeCluster, KubeClusterInfo, KubeClusterStatus,
        KubeContainerImage, KubeEnvironment, KubeNamespace, KubeNamespaceInfo, KubeWorkload,
        KubeWorkloadKind, KubeWorkloadList, PagePaginationParams, PaginatedInfo, PaginatedResult,
        PaginationParams,
    },
    token::PlainToken,
    utils::rand_string,
};
use lapdev_db::api::DbApi;
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube::tunnel::TunnelRegistry;
use lapdev_rpc::error::ApiError;
use sea_orm::TransactionTrait;
use secrecy::ExposeSecret;

#[derive(Clone)]
pub struct KubeController {
    // KubeManager connections per cluster
    pub kube_cluster_servers: Arc<RwLock<HashMap<Uuid, Vec<KubeClusterServer>>>>,
    // Tunnel registry for preview URL functionality
    pub tunnel_registry: Arc<TunnelRegistry>,
    // Database API
    pub db: DbApi,
}

impl KubeController {
    pub fn new(db: DbApi) -> Self {
        Self {
            kube_cluster_servers: Arc::new(RwLock::new(HashMap::new())),
            tunnel_registry: Arc::new(TunnelRegistry::new()),
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
                can_deploy_personal: c.can_deploy_personal,
                can_deploy_shared: c.can_deploy_shared,
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
                "Cannot delete cluster: it has active app catalogs. Please delete them first."
                    .to_string(),
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
                "Cannot delete cluster: it has active environments. Please delete them first."
                    .to_string(),
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
        can_deploy_personal: bool,
        can_deploy_shared: bool,
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

        // Update the deployment capability fields
        self.db
            .set_cluster_deployable(cluster_id, can_deploy_personal, can_deploy_shared)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn set_cluster_personal_deployable(
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

        // Update only the personal deployment capability
        self.db
            .set_cluster_deployable(cluster_id, can_deploy, cluster.can_deploy_shared)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn set_cluster_shared_deployable(
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

        // Update only the shared deployment capability
        self.db
            .set_cluster_deployable(cluster_id, cluster.can_deploy_personal, can_deploy)
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

    pub async fn get_cluster_namespaces(
        &self,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespaceInfo>, ApiError> {
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

    async fn enrich_workloads_with_details(
        &self,
        cluster_id: Uuid,
        workloads: Vec<lapdev_common::kube::KubeAppCatalogWorkloadCreate>,
    ) -> Result<Vec<lapdev_common::kube::KubeWorkloadDetails>, ApiError> {
        // Get cluster connection
        let cluster_server = self
            .get_random_kube_cluster_server(cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Convert to WorkloadIdentifier for RPC call
        let workload_identifiers: Vec<lapdev_kube_rpc::WorkloadIdentifier> = workloads
            .iter()
            .map(|w| lapdev_kube_rpc::WorkloadIdentifier {
                name: w.name.clone(),
                namespace: w.namespace.clone(),
                kind: w.kind.clone(),
            })
            .collect();

        // Get detailed information from KubeManager
        let workload_details = cluster_server
            .rpc_client
            .get_workloads_details(tarpc::context::current(), workload_identifiers)
            .await
            .map_err(|e| {
                ApiError::InvalidRequest(format!("Failed to get workload details: {}", e))
            })?;

        workload_details.map_err(|e| ApiError::InvalidRequest(format!("KubeManager error: {}", e)))
    }

    pub async fn create_app_catalog(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        workloads: Vec<lapdev_common::kube::KubeAppCatalogWorkloadCreate>,
    ) -> Result<Uuid, ApiError> {
        // Get enriched workload details from KubeManager
        let enriched_workloads = self
            .enrich_workloads_with_details(cluster_id, workloads)
            .await?;

        self.db
            .create_app_catalog_with_enriched_workloads(
                org_id,
                user_id,
                cluster_id,
                name,
                description,
                enriched_workloads,
            )
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

        // TODO: Clean up Kubernetes resources from the cluster
        // Get cluster server to perform cleanup if needed
        if let Some(_cluster_server) = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
        {
            tracing::info!(
                "Deleting environment '{}' in namespace '{}'",
                environment.name,
                environment.namespace
            );
            // TODO: Implement actual K8s resource cleanup
        }

        // Delete from database (soft delete)
        self.db
            .delete_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    pub async fn get_app_catalog(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<KubeAppCatalog, ApiError> {
        // Get catalog with cluster info
        let (catalog, cluster) = self
            .db
            .get_all_app_catalogs_paginated(org_id, None, None)
            .await
            .map_err(ApiError::from)?
            .0
            .into_iter()
            .find(|(cat, _)| cat.id == catalog_id)
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        let cluster =
            cluster.ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        Ok(KubeAppCatalog {
            id: catalog.id,
            name: catalog.name,
            description: catalog.description,
            created_at: catalog.created_at,
            created_by: catalog.created_by,
            cluster_id: catalog.cluster_id,
            cluster_name: cluster.name,
        })
    }

    pub async fn get_app_catalog_workloads(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeAppCatalogWorkload>, ApiError> {
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

        self.db
            .get_app_catalog_workloads(catalog_id)
            .await
            .map_err(ApiError::from)
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
                "Cannot delete app catalog: it has active environments. Please delete them first."
                    .to_string(),
            ));
        }

        // Soft delete the app catalog
        self.db
            .delete_app_catalog(catalog_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn delete_app_catalog_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<(), ApiError> {
        // First get the workload to find its catalog
        let workload = self
            .db
            .get_app_catalog_workload(workload_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Workload not found".to_string()))?;

        // Verify the catalog belongs to the organization
        let catalog = self
            .db
            .get_app_catalog(workload.app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Delete the workload
        self.db
            .delete_app_catalog_workload(workload_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn update_app_catalog_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
    ) -> Result<(), ApiError> {
        // Validate containers
        Self::validate_containers(&containers)?;

        // First get the workload to find its catalog
        let workload = self
            .db
            .get_app_catalog_workload(workload_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Workload not found".to_string()))?;

        // Verify the catalog belongs to the organization
        let catalog = self
            .db
            .get_app_catalog(workload.app_catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Update the workload containers
        self.db
            .update_app_catalog_workload(workload_id, containers)
            .await
            .map_err(ApiError::from)
    }

    pub async fn add_workloads_to_app_catalog(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        catalog_id: Uuid,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<(), ApiError> {
        // Verify the catalog belongs to the organization
        let catalog = self
            .db
            .get_app_catalog(catalog_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // Enrich workloads with details from KubeManager
        let enriched_workloads = self
            .enrich_workloads_with_details(catalog.cluster_id, workloads)
            .await?;

        // Add enriched workloads to the catalog
        let txn = self.db.conn.begin().await.map_err(ApiError::from)?;
        let now = chrono::Utc::now().into();

        match self
            .db
            .insert_enriched_workloads_to_catalog(&txn, catalog_id, enriched_workloads, now)
            .await
        {
            Ok(_) => {
                txn.commit().await.map_err(ApiError::from)?;
                Ok(())
            }
            Err(db_err) => {
                txn.rollback().await.map_err(ApiError::from)?;
                // Check if this is a unique constraint violation using SeaORM's sql_err() method
                if matches!(
                    db_err.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    Err(ApiError::InvalidRequest(
                        "One or more selected workloads already exist in this catalog".to_string(),
                    ))
                } else {
                    Err(ApiError::from(anyhow::Error::from(db_err)))
                }
            }
        }
    }

    pub async fn create_kube_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
        is_shared: bool,
    ) -> Result<lapdev_common::kube::KubeEnvironment, ApiError> {
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

        // Store the environment workloads in the database before deployment
        let workload_details: Vec<lapdev_common::kube::KubeWorkloadDetails> = workloads
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
                }
            })
            .collect();

        // Create the environment, workloads, and services in a single transaction
        let created_env = match self
            .db
            .create_kube_environment(
                org_id,
                user_id,
                app_catalog_id,
                cluster_id,
                name.clone(),
                namespace.clone(),
                Some("Pending".to_string()),
                is_shared,
                None, // No base environment for regular environments
                workload_details,
                workloads_with_resources.services.clone(),
            )
            .await
        {
            Ok(env) => env,
            Err(db_err) => {
                // Check if this is a unique constraint violation for app catalog + cluster + namespace
                if matches!(
                    db_err.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    return Err(ApiError::InvalidRequest(
                        "This app catalog is already deployed to the specified namespace in this cluster".to_string(),
                    ));
                } else {
                    return Err(ApiError::from(anyhow::Error::from(db_err)));
                }
            }
        };

        // Deploy the app catalog resources to the cluster using pre-fetched YAML
        self.deploy_app_catalog_with_yaml(
            &server,
            &namespace,
            &name,
            created_env.id,
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
        })
    }

    pub async fn create_branch_environment(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        base_environment_id: Uuid,
        name: String,
    ) -> Result<lapdev_common::kube::KubeEnvironment, ApiError> {
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

        // Convert workloads to the format needed for database creation
        let workload_details: Vec<lapdev_common::kube::KubeWorkloadDetails> = base_workloads
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
                        namespace: base_environment.namespace.clone(),
                        kind,
                        containers,
                    }
                })
            })
            .collect();

        // Convert services to the format needed for database creation
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

        // Create the branch environment in the database
        let created_env = self
            .db
            .create_kube_environment(
                org_id,
                user_id,
                base_environment.app_catalog_id,
                base_environment.cluster_id,
                name.clone(),
                base_environment.namespace.clone(), // Use same namespace as base
                Some("Pending".to_string()),
                false, // Branch environments are always personal (not shared)
                Some(base_environment_id), // Set the base environment reference
                workload_details,
                services_map,
            )
            .await
            .map_err(ApiError::from)?;

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
        })
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
        org_id: Uuid,
        namespace_id: Uuid,
    ) -> Result<(), ApiError> {
        self.db
            .delete_kube_namespace(namespace_id)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    async fn get_workloads_yaml_for_catalog(
        &self,
        app_catalog: &lapdev_db_entities::kube_app_catalog::Model,
        workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<lapdev_kube_rpc::KubeWorkloadsWithResources, ApiError> {
        let source_server = self
            .get_random_kube_cluster_server(app_catalog.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest(
                    "No connected KubeManager for the app catalog's source cluster".to_string(),
                )
            })?;

        match source_server
            .rpc_client
            .get_workloads_yaml(tarpc::context::current(), workloads)
            .await
        {
            Ok(Ok(workloads_with_resources)) => Ok(workloads_with_resources),
            Ok(Err(e)) => {
                tracing::error!(
                    "Failed to get YAML for workloads from source cluster: {}",
                    e
                );
                Err(ApiError::InvalidRequest(format!(
                    "Failed to get YAML for workloads from source cluster: {e}"
                )))
            }
            Err(e) => Err(ApiError::InvalidRequest(format!(
                "Connection error to source cluster: {e}"
            ))),
        }
    }

    async fn deploy_app_catalog_with_yaml(
        &self,
        target_server: &KubeClusterServer,
        namespace: &str,
        environment_name: &str,
        environment_id: Uuid,
        workloads_with_resources: lapdev_kube_rpc::KubeWorkloadsWithResources,
    ) -> Result<(), ApiError> {
        tracing::info!(
            "Deploying app catalog resources for environment '{}' in namespace '{}'",
            environment_name,
            namespace
        );

        if workloads_with_resources.workloads.is_empty() {
            tracing::warn!("No workloads found for environment '{}'", environment_name);
            return Ok(());
        }

        tracing::info!(
            "Found {} workloads to deploy for environment '{}'",
            workloads_with_resources.workloads.len(),
            environment_name
        );

        // Prepare environment-specific labels
        let mut environment_labels = std::collections::HashMap::new();
        environment_labels.insert(
            "lapdev.environment".to_string(),
            environment_name.to_string(),
        );
        environment_labels.insert("lapdev.managed-by".to_string(), "lapdev".to_string());

        // Deploy all workloads and resources in a single call
        match target_server
            .rpc_client
            .deploy_workload_yaml(
                tarpc::context::current(),
                environment_id,
                namespace.to_string(),
                workloads_with_resources,
                environment_labels.clone(),
            )
            .await
        {
            Ok(Ok(())) => {
                tracing::info!("Successfully deployed all workloads to target cluster");
                Ok(())
            }
            Ok(Err(e)) => {
                tracing::error!("Failed to deploy workloads to target cluster: {}", e);
                Err(ApiError::InvalidRequest(format!(
                    "Failed to deploy workloads to target cluster: {e}"
                )))
            }
            Err(e) => Err(ApiError::InvalidRequest(format!(
                "Connection error to target cluster: {e}"
            ))),
        }
    }

    fn validate_containers(
        containers: &[lapdev_common::kube::KubeContainerInfo],
    ) -> Result<(), ApiError> {
        if containers.is_empty() {
            return Err(ApiError::InvalidRequest(
                "At least one container is required".to_string(),
            ));
        }

        for (index, container) in containers.iter().enumerate() {
            if container.name.trim().is_empty() {
                return Err(ApiError::InvalidRequest(format!(
                    "Container {} name cannot be empty",
                    index + 1
                )));
            }

            match &container.image {
                KubeContainerImage::Custom(image) => {
                    if image.trim().is_empty() {
                        return Err(ApiError::InvalidRequest(format!(
                            "Container '{}' custom image cannot be empty",
                            container.name
                        )));
                    }
                }
                KubeContainerImage::FollowOriginal => {
                    // No validation needed for FollowOriginal
                }
            }

            // Validate CPU resources
            if let Some(cpu_request) = &container.cpu_request {
                if !Self::is_valid_cpu_quantity(cpu_request) {
                    return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid CPU request format. Use formats like '100m', '0.1', '1'. Minimum precision is 1m (0.001 CPU)", container.name)));
                }
            }

            if let Some(cpu_limit) = &container.cpu_limit {
                if !Self::is_valid_cpu_quantity(cpu_limit) {
                    return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid CPU limit format. Use formats like '100m', '0.1', '1'. Minimum precision is 1m (0.001 CPU)", container.name)));
                }
            }

            // Validate memory resources
            if let Some(memory_request) = &container.memory_request {
                if !Self::is_valid_memory_quantity(memory_request) {
                    return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid memory request format. Use formats like '128Mi', '1Gi', '512M'. Maximum 3 decimal places allowed", container.name)));
                }
            }

            if let Some(memory_limit) = &container.memory_limit {
                if !Self::is_valid_memory_quantity(memory_limit) {
                    return Err(ApiError::InvalidRequest(format!("Container '{}' has invalid memory limit format. Use formats like '128Mi', '1Gi', '512M'. Maximum 3 decimal places allowed", container.name)));
                }
            }

            // Validate environment variables
            for env_var in &container.env_vars {
                if env_var.name.trim().is_empty() {
                    return Err(ApiError::InvalidRequest(format!(
                        "Container '{}' has environment variable with empty name",
                        container.name
                    )));
                }
            }
        }

        Ok(())
    }

    fn is_valid_cpu_quantity(quantity: &str) -> bool {
        // CPU validation for Kubernetes
        // Supports formats like: 100m, 0.1, 1, 2.5, etc.
        // Kubernetes minimum precision is 1m (0.001 CPU)
        if quantity.trim().is_empty() {
            return false; // Empty values are invalid - must specify a resource amount
        }

        let quantity = quantity.trim();

        // CPU can have 'm' suffix for millicores or be a plain decimal number
        let re = match regex::Regex::new(r"^(\d+\.?\d*|\.\d+)m?$") {
            Ok(regex) => regex,
            Err(_) => return false,
        };

        if !re.is_match(quantity) {
            return false;
        }

        // Parse the numeric part and validate precision constraints
        let (numeric_part, has_millicore_suffix) = if quantity.ends_with('m') {
            (&quantity[..quantity.len() - 1], true)
        } else {
            (quantity, false)
        };

        if let Ok(value) = numeric_part.parse::<f64>() {
            if value <= 0.0 {
                return false; // Must be positive
            }

            if has_millicore_suffix {
                // For millicores (m suffix), minimum is 1m, so value must be >= 1.0
                value >= 1.0
            } else {
                // For plain decimal CPU values, minimum precision is 0.001 (1m equivalent)
                value >= 0.001
            }
        } else {
            false
        }
    }

    fn is_valid_memory_quantity(quantity: &str) -> bool {
        // Memory validation for Kubernetes
        // Supports formats like: 128Mi, 1Gi, 512M, 1000000000, etc.
        // Maximum precision is 3 decimal places
        if quantity.trim().is_empty() {
            return false; // Empty values are invalid - must specify a resource amount
        }

        let quantity = quantity.trim();

        // Memory can have binary (Ki, Mi, Gi, Ti, Pi, Ei) or decimal (K, M, G, T, P, E) suffixes
        let valid_suffixes = [
            "Ki", "Mi", "Gi", "Ti", "Pi", "Ei", "K", "M", "G", "T", "P", "E",
        ];

        let (numeric_part, _suffix) =
            if let Some(suffix) = valid_suffixes.iter().find(|&s| quantity.ends_with(s)) {
                (&quantity[..quantity.len() - suffix.len()], Some(*suffix))
            } else {
                // No suffix means it's in bytes
                (quantity, None)
            };

        // Validate the numeric part is a positive number with max 3 decimal places
        if let Ok(value) = numeric_part.parse::<f64>() {
            if value <= 0.0 {
                return false; // Must be positive
            }

            // Check decimal places constraint (max 3 decimal places)
            if let Some(decimal_pos) = numeric_part.find('.') {
                let decimal_part = &numeric_part[decimal_pos + 1..];
                if decimal_part.len() > 3 {
                    return false; // More than 3 decimal places
                }
            }

            true
        } else {
            false
        }
    }

    pub async fn get_environment_workloads(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentWorkload>, ApiError> {
        // Verify environment belongs to the organization
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

        self.db
            .get_environment_workloads(environment_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn get_environment_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<Option<lapdev_common::kube::KubeEnvironmentWorkload>, ApiError> {
        // First get the workload to find its environment
        if let Some(workload) = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)?
        {
            // Verify the environment belongs to the organization
            let environment = self
                .db
                .get_kube_environment(workload.environment_id)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;
            if environment.organization_id != org_id {
                return Err(ApiError::Unauthorized);
            }
            Ok(Some(workload))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_environment_workload(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        workload_id: Uuid,
        environment: lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), ApiError> {
        // Verify the environment belongs to the organization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // For personal/branch environments, verify ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Delete the workload
        self.db
            .delete_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn update_environment_workload(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        environment: lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), ApiError> {
        // Verify the environment belongs to the organization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // For personal/branch environments, verify ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Update the workload containers in database
        let updated_db_model = self
            .db
            .update_environment_workload(workload_id, containers)
            .await
            .map_err(ApiError::from)?;

        // Convert database model to API type
        let updated_workload = {
            let containers: Vec<lapdev_common::kube::KubeContainerInfo> =
                serde_json::from_value(updated_db_model.containers.clone()).unwrap_or_default();

            lapdev_common::kube::KubeEnvironmentWorkload {
                id: updated_db_model.id,
                created_at: updated_db_model.created_at,
                environment_id: updated_db_model.environment_id,
                name: updated_db_model.name.clone(),
                namespace: updated_db_model.namespace.clone(),
                kind: updated_db_model.kind.clone(),
                containers,
            }
        };

        // After successful database update, deploy the workload to the cluster
        let cluster_server = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        // Convert to proper workload kind
        let workload_kind = updated_workload.kind.parse().map_err(|_| {
            ApiError::InvalidRequest(format!("Invalid workload kind: {}", updated_workload.kind))
        })?;

        // Prepare environment-specific labels
        let mut environment_labels = std::collections::HashMap::new();
        environment_labels.insert("lapdev.environment".to_string(), environment.name.clone());
        environment_labels.insert("lapdev.managed-by".to_string(), "lapdev".to_string());

        // Check if this is a branch environment - if so, create a new deployment
        if environment.base_environment_id.is_some() {
            tracing::info!(
                "Creating branch deployment for workload '{}' in branch environment '{}' (namespace '{}')",
                updated_workload.name,
                environment.name,
                environment.namespace
            );

            // For branch environments, we need to get the base workload name and create a branch deployment
            // The base workload name is the original workload name without branch suffix
            let base_workload_name = updated_workload.name.clone();
            let branch_environment_id = environment.id;

            match cluster_server
                .rpc_client
                .create_branch_workload(
                    tarpc::context::current(),
                    updated_workload.id,
                    base_workload_name.clone(),
                    branch_environment_id,
                    updated_workload.namespace.clone(),
                    workload_kind,
                    updated_workload.containers,
                    environment_labels,
                )
                .await
            {
                Ok(Ok(())) => {
                    tracing::info!(
                        "Successfully created branch deployment for workload '{}' in branch environment '{}' (namespace '{}')",
                        updated_workload.name,
                        environment.name,
                        environment.namespace
                    );
                    Ok(())
                }
                Ok(Err(e)) => {
                    tracing::error!("Failed to create branch deployment: {}", e);
                    Err(ApiError::InvalidRequest(format!(
                        "Failed to create branch deployment: {e}"
                    )))
                }
                Err(e) => Err(ApiError::InvalidRequest(format!(
                    "Connection error during branch deployment creation: {e}"
                ))),
            }
        } else {
            // For regular environments, update the existing workload containers
            tracing::info!(
                "Updating workload containers for '{}' in regular environment '{}' (namespace '{}')",
                updated_workload.name,
                environment.name,
                environment.namespace
            );

            match cluster_server
                .rpc_client
                .update_workload_containers(
                    tarpc::context::current(),
                    environment.id,
                    updated_workload.id,
                    updated_workload.name.clone(),
                    updated_workload.namespace.clone(),
                    workload_kind,
                    updated_workload.containers,
                    environment_labels,
                )
                .await
            {
                Ok(Ok(())) => {
                    tracing::info!(
                        "Successfully updated workload containers for '{}' in environment '{}' (namespace '{}')",
                        updated_workload.name,
                        environment.name,
                        environment.namespace
                    );
                    Ok(())
                }
                Ok(Err(e)) => {
                    tracing::error!("Failed to atomically update workload containers: {}", e);
                    Err(ApiError::InvalidRequest(format!(
                        "Failed to update workload containers: {e}"
                    )))
                }
                Err(e) => Err(ApiError::InvalidRequest(format!(
                    "Connection error during atomic workload update: {e}"
                ))),
            }
        }
    }

    pub async fn get_environment_services(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentService>, ApiError> {
        // Verify environment belongs to the organization
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

        self.db
            .get_environment_services(environment_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn create_environment_preview_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
        request: lapdev_common::kube::CreateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<lapdev_common::kube::KubeEnvironmentPreviewUrl, ApiError> {
        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Verify service exists and belongs to the environment
        let service = self
            .db
            .get_kube_environment_service_by_id(request.service_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Service not found".to_string()))?;

        // Verify service belongs to the environment
        if service.environment_id != environment_id {
            return Err(ApiError::InvalidRequest(
                "Service not found in environment".to_string(),
            ));
        }

        // Validate the port exists on the service
        let port_exists = service.ports.iter().any(|p| {
            p.port == request.port
                && (request.port_name.is_none() || p.name.as_ref() == request.port_name.as_ref())
        });

        if !port_exists {
            return Err(ApiError::InvalidRequest(
                "Specified port does not exist on the service".to_string(),
            ));
        }

        // Auto-generate name based on service and port
        let auto_name = format!("{}-{}-{}", request.port, service.name, rand_string(12));

        // Set defaults
        let protocol = request.protocol.unwrap_or_else(|| "HTTP".to_string());
        let access_level = request
            .access_level
            .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Personal);

        let url = format!("https://{auto_name}.app.lap.dev");

        // Create preview URL in database
        let preview_url = match self
            .db
            .create_environment_preview_url(
                environment_id,
                request.service_id,
                user_id,
                auto_name,
                request.description,
                request.port,
                request.port_name,
                protocol.clone(),
                access_level.clone(),
            )
            .await
        {
            Ok(url) => url,
            Err(db_err) => {
                // Check if this is a unique constraint violation
                if matches!(
                    db_err.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    return Err(ApiError::InvalidRequest(
                        "A preview URL already exists for this service and port combination"
                            .to_string(),
                    ));
                } else {
                    return Err(ApiError::from(anyhow::Error::from(db_err)));
                }
            }
        };

        Ok(lapdev_common::kube::KubeEnvironmentPreviewUrl {
            id: preview_url.id,
            created_at: preview_url.created_at,
            environment_id: preview_url.environment_id,
            service_id: preview_url.service_id,
            name: preview_url.name,
            description: preview_url.description,
            port: preview_url.port,
            port_name: preview_url.port_name,
            protocol: preview_url.protocol,
            access_level: preview_url
                .access_level
                .parse()
                .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Personal),
            created_by: preview_url.created_by,
            last_accessed_at: preview_url.last_accessed_at,
            url,
        })
    }

    pub async fn get_environment_preview_urls(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentPreviewUrl>, ApiError> {
        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        let preview_urls = self
            .db
            .get_environment_preview_urls(environment_id)
            .await
            .map_err(ApiError::from)?;

        Ok(preview_urls
            .into_iter()
            .map(|preview_url| {
                let url = format!("https://{}.app.lap.dev", preview_url.name);

                lapdev_common::kube::KubeEnvironmentPreviewUrl {
                    id: preview_url.id,
                    created_at: preview_url.created_at,
                    environment_id: preview_url.environment_id,
                    service_id: preview_url.service_id,
                    name: preview_url.name,
                    description: preview_url.description,
                    port: preview_url.port,
                    port_name: preview_url.port_name,
                    protocol: preview_url.protocol,
                    access_level: preview_url
                        .access_level
                        .parse()
                        .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Personal),
                    created_by: preview_url.created_by,
                    last_accessed_at: preview_url.last_accessed_at,
                    url,
                }
            })
            .collect())
    }

    pub async fn update_environment_preview_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        preview_url_id: Uuid,
        request: lapdev_common::kube::UpdateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<lapdev_common::kube::KubeEnvironmentPreviewUrl, ApiError> {
        // Get the preview URL and verify ownership
        let preview_url = self
            .db
            .get_environment_preview_url(preview_url_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Preview URL not found".to_string()))?;

        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(preview_url.environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Update the preview URL
        let updated_preview_url = self
            .db
            .update_environment_preview_url(
                preview_url_id,
                request.description,
                request.access_level,
            )
            .await
            .map_err(ApiError::from)?;

        let url = format!("https://{}.app.lap.dev", updated_preview_url.name);

        Ok(lapdev_common::kube::KubeEnvironmentPreviewUrl {
            id: updated_preview_url.id,
            created_at: updated_preview_url.created_at,
            environment_id: updated_preview_url.environment_id,
            service_id: updated_preview_url.service_id,
            name: updated_preview_url.name,
            description: updated_preview_url.description,
            port: updated_preview_url.port,
            port_name: updated_preview_url.port_name,
            protocol: updated_preview_url.protocol,
            access_level: updated_preview_url
                .access_level
                .parse()
                .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Personal),
            created_by: updated_preview_url.created_by,
            last_accessed_at: updated_preview_url.last_accessed_at,
            url,
        })
    }

    pub async fn delete_environment_preview_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        preview_url_id: Uuid,
    ) -> Result<(), ApiError> {
        // Get the preview URL and verify ownership
        let preview_url = self
            .db
            .get_environment_preview_url(preview_url_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Preview URL not found".to_string()))?;

        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(preview_url.environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        self.db
            .delete_environment_preview_url(preview_url_id)
            .await
            .map_err(ApiError::from)
    }
}

#[cfg(test)]
mod tests {
    use super::KubeController;
    use lapdev_common::kube::{KubeContainerImage, KubeContainerInfo};

    #[test]
    fn test_cpu_validation() {
        // Valid CPU values
        assert!(KubeController::is_valid_cpu_quantity("100m")); // 100 millicores
        assert!(KubeController::is_valid_cpu_quantity("1m")); // 1 millicore (minimum)
        assert!(KubeController::is_valid_cpu_quantity("0.1")); // 0.1 CPU (100m equivalent)
        assert!(KubeController::is_valid_cpu_quantity("1")); // 1 CPU
        assert!(KubeController::is_valid_cpu_quantity("2.5")); // 2.5 CPUs
        assert!(KubeController::is_valid_cpu_quantity("500m")); // 500 millicores
        assert!(KubeController::is_valid_cpu_quantity("1.5m")); // 1.5 millicores
        assert!(KubeController::is_valid_cpu_quantity("0.001")); // Minimum decimal precision

        // Invalid CPU values
        assert!(!KubeController::is_valid_cpu_quantity("")); // Empty
        assert!(!KubeController::is_valid_cpu_quantity("   ")); // Whitespace
        assert!(!KubeController::is_valid_cpu_quantity("0.5m")); // Below 1m minimum
        assert!(!KubeController::is_valid_cpu_quantity("0.0005")); // Below 0.001 minimum
        assert!(!KubeController::is_valid_cpu_quantity("0m")); // Zero millicores
        assert!(!KubeController::is_valid_cpu_quantity("0")); // Zero CPU
        assert!(!KubeController::is_valid_cpu_quantity("100Mi")); // Wrong suffix
        assert!(!KubeController::is_valid_cpu_quantity("-100m")); // Negative
        assert!(!KubeController::is_valid_cpu_quantity("abc")); // Non-numeric
        assert!(!KubeController::is_valid_cpu_quantity("100x")); // Invalid suffix
    }

    #[test]
    fn test_memory_validation() {
        // Valid memory values
        assert!(KubeController::is_valid_memory_quantity("128Mi"));
        assert!(KubeController::is_valid_memory_quantity("1Gi"));
        assert!(KubeController::is_valid_memory_quantity("512M"));
        assert!(KubeController::is_valid_memory_quantity("1000000000")); // Raw bytes
        assert!(KubeController::is_valid_memory_quantity("2Ti"));
        assert!(KubeController::is_valid_memory_quantity("1.5")); // 1 decimal place
        assert!(KubeController::is_valid_memory_quantity("1.5Gi")); // 1 decimal place
        assert!(KubeController::is_valid_memory_quantity("128.25Mi")); // 2 decimal places
        assert!(KubeController::is_valid_memory_quantity("1.125Gi")); // 3 decimal places (max)
        assert!(KubeController::is_valid_memory_quantity("0.5Gi")); // Decimal with suffix

        // Invalid memory values
        assert!(!KubeController::is_valid_memory_quantity("")); // Empty
        assert!(!KubeController::is_valid_memory_quantity("   ")); // Whitespace
        assert!(!KubeController::is_valid_memory_quantity("100m")); // CPU suffix on memory
        assert!(!KubeController::is_valid_memory_quantity("-128Mi")); // Negative
        assert!(!KubeController::is_valid_memory_quantity("abc")); // Non-numeric
        assert!(!KubeController::is_valid_memory_quantity("100x")); // Invalid suffix
        assert!(!KubeController::is_valid_memory_quantity("0Mi")); // Zero
        assert!(!KubeController::is_valid_memory_quantity("1.1234Gi")); // 4 decimal places (too many)
        assert!(!KubeController::is_valid_memory_quantity("1.1234")); // 4 decimal places (too many)
        assert!(!KubeController::is_valid_memory_quantity("128.12345Mi")); // 5 decimal places (too many)
        assert!(!KubeController::is_valid_memory_quantity("128.12345")); // 5 decimal places (too many)
    }
}
