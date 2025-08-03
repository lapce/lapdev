use std::str::FromStr;
use anyhow::Result;
use chrono::Utc;
use lapdev_api_hrpc::HrpcService;
use lapdev_common::{
    hrpc::HrpcError,
    kube::{
        CreateKubeClusterResponse, KubeAppCatalog, KubeCluster, KubeClusterInfo,
        KubeClusterStatus, KubeEnvironment, KubeNamespace, KubeWorkload, KubeWorkloadKind,
        KubeWorkloadList, PagePaginationParams, PaginatedInfo, PaginatedResult, PaginationParams,
    },
    token::PlainToken,
    UserRole,
};
use lapdev_rpc::error::ApiError;
use sea_orm::{
    ActiveModelTrait, ActiveValue, ColumnTrait, EntityTrait, QueryFilter, TransactionTrait,
};
use sea_orm::{PaginatorTrait, QueryOrder, QuerySelect};
use secrecy::ExposeSecret;
use uuid::Uuid;

use crate::state::CoreState;

impl HrpcService for CoreState {
    async fn all_kube_clusters(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
    ) -> Result<Vec<KubeCluster>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;
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
                    provider: None, // TODO: Get provider info
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

    async fn create_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        let cluster_id = Uuid::new_v4();
        let token = PlainToken::generate();
        let hashed_token = token.hashed();
        let token_name = format!("{name}-default");

        // Use a transaction to ensure both cluster and token are created atomically
        let txn = self.db.conn.begin().await.map_err(ApiError::from)?;

        // Create the cluster
        lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            cluster_version: ActiveValue::Set(None),
            status: ActiveValue::Set(Some(KubeClusterStatus::Provisioning.to_string())),
            region: ActiveValue::Set(None),
            created_by: ActiveValue::Set(user.id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
            last_reported_at: ActiveValue::Set(None),
            can_deploy: ActiveValue::Set(true),
        }
        .insert(&txn)
        .await
        .map_err(ApiError::from)?;

        // Create the cluster token
        lapdev_db_entities::kube_cluster_token::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()), // UUID primary key
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            last_used_at: ActiveValue::Set(None),
            cluster_id: ActiveValue::Set(cluster_id), // Note: typo in entity field name
            created_by: ActiveValue::Set(user.id),
            name: ActiveValue::Set(token_name),
            token: ActiveValue::Set(hashed_token.expose_secret().to_vec()),
        }
        .insert(&txn)
        .await
        .map_err(ApiError::from)?;

        txn.commit().await.map_err(ApiError::from)?;

        Ok(CreateKubeClusterResponse {
            cluster_id,
            token: token.expose_secret().to_string(),
        })
    }

    async fn delete_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Check for dependencies - kube_app_catalog
        let has_app_catalogs = lapdev_db_entities::kube_app_catalog::Entity::find()
            .filter(lapdev_db_entities::kube_app_catalog::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await
            .map_err(ApiError::from)?
            .is_some();

        if has_app_catalogs {
            return Err(ApiError::InvalidRequest(
                "Cannot delete cluster: it has active app catalogs. Please delete them first."
                    .to_string(),
            )
            .into());
        }

        // Check for dependencies - kube_environment
        let has_environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await
            .map_err(ApiError::from)?
            .is_some();

        if has_environments {
            return Err(ApiError::InvalidRequest(
                "Cannot delete cluster: it has active environments. Please delete them first."
                    .to_string(),
            )
            .into());
        }

        // Soft delete by setting deleted_at timestamp
        lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster_id),
            deleted_at: ActiveValue::Set(Some(Utc::now().into())),
            ..Default::default()
        }
        .update(&self.db.conn)
        .await
        .map_err(ApiError::from)?;

        Ok(())
    }

    async fn set_cluster_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError> {
        let _ = self.authorize(headers, org_id, Some(UserRole::Admin)).await?;
        
        // Find the cluster
        let cluster = lapdev_db_entities::kube_cluster::Entity::find_by_id(cluster_id)
            .one(&self.db.conn)
            .await
            .map_err(ApiError::from)?;

        let cluster = cluster.ok_or(ApiError::InvalidRequest("Cluster not found".to_string()))?;

        // Verify the cluster belongs to the organization
        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Update the can_deploy field
        let active_model = lapdev_db_entities::kube_cluster::ActiveModel {
            id: ActiveValue::Set(cluster.id),
            can_deploy: ActiveValue::Set(can_deploy),
            ..Default::default()
        };
        
        active_model.update(&self.db.conn).await.map_err(ApiError::from)?;

        Ok(())
    }

    async fn get_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .kube_controller
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
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!("KubeManager error: {}", e)).into()),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e)).into()),
        }
    }

    async fn get_workload_details(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .kube_controller
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
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!("KubeManager error: {}", e)).into()),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e)).into()),
        }
    }

    async fn get_namespaces(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespace>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Get a connected KubeClusterServer for this cluster
        let server = self
            .kube_controller
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
            Ok(Err(e)) => Err(ApiError::InvalidRequest(format!("KubeManager error: {}", e)).into()),
            Err(e) => Err(ApiError::InvalidRequest(format!("Connection error: {}", e)).into()),
        }
    }

    async fn get_cluster_info(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeClusterInfo, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        // Get cluster from database
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
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

    async fn create_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        resources: String,
    ) -> Result<(), HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        lapdev_db_entities::kube_app_catalog::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            name: ActiveValue::Set(name),
            description: ActiveValue::Set(description),
            resources: ActiveValue::Set(resources),
            cluster_id: ActiveValue::Set(cluster_id),
            created_by: ActiveValue::Set(user.id),
            organization_id: ActiveValue::Set(org_id),
            deleted_at: ActiveValue::Set(None),
        }
        .insert(&self.db.conn)
        .await
        .map_err(ApiError::from)?;

        Ok(())
    }

    async fn all_app_catalogs(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeAppCatalog>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        let pagination = pagination.unwrap_or_default();

        // Build base query
        let mut base_query = lapdev_db_entities::kube_app_catalog::Entity::find()
            .filter(lapdev_db_entities::kube_app_catalog::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null());

        if let Some(search_term) = search.as_ref().filter(|s| !s.trim().is_empty()) {
            use sea_orm::sea_query::{extension::postgres::PgExpr, Expr, SimpleExpr};
            let search_pattern = format!("%{}%", search_term.trim().to_lowercase());
            base_query = base_query.filter(
                SimpleExpr::FunctionCall(sea_orm::sea_query::Func::lower(Expr::col((
                    lapdev_db_entities::kube_app_catalog::Entity,
                    lapdev_db_entities::kube_app_catalog::Column::Name,
                ))))
                .ilike(&search_pattern),
            );
        }

        // Get total count
        let total_count = base_query
            .clone()
            .count(&self.db.conn)
            .await
            .map_err(ApiError::from)? as usize;

        // Apply pagination
        let offset = (pagination.page.saturating_sub(1)) * pagination.page_size;
        let paginated_query = base_query
            .limit(pagination.page_size as u64)
            .offset(offset as u64)
            .order_by_desc(lapdev_db_entities::kube_app_catalog::Column::OrganizationId)
            .order_by_desc(lapdev_db_entities::kube_app_catalog::Column::DeletedAt)
            .order_by_desc(lapdev_db_entities::kube_app_catalog::Column::CreatedAt);

        let catalogs_with_clusters = paginated_query
            .find_also_related(lapdev_db_entities::kube_cluster::Entity)
            .all(&self.db.conn)
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

    async fn all_kube_environments(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        let pagination = pagination.unwrap_or_default();

        // Build base query - filter by user ID so users only see their own environments
        let mut base_query = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::OrganizationId.eq(org_id))
            .filter(lapdev_db_entities::kube_environment::Column::UserId.eq(user.id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null());

        if let Some(search_term) = search.as_ref().filter(|s| !s.trim().is_empty()) {
            use sea_orm::sea_query::{extension::postgres::PgExpr, Expr, SimpleExpr};
            let search_pattern = format!("%{}%", search_term.trim().to_lowercase());
            base_query = base_query.filter(
                SimpleExpr::FunctionCall(sea_orm::sea_query::Func::lower(Expr::col((
                    lapdev_db_entities::kube_environment::Entity,
                    lapdev_db_entities::kube_environment::Column::Name,
                ))))
                .ilike(&search_pattern),
            );
        }

        // Get total count
        let total_count = base_query
            .clone()
            .count(&self.db.conn)
            .await
            .map_err(ApiError::from)? as usize;

        // Apply pagination
        let offset = (pagination.page.saturating_sub(1)) * pagination.page_size;
        let paginated_query = base_query
            .limit(pagination.page_size as u64)
            .offset(offset as u64)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::OrganizationId)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::UserId)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::DeletedAt)
            .order_by_desc(lapdev_db_entities::kube_environment::Column::CreatedAt);

        let environments_with_catalogs = paginated_query
            .find_also_related(lapdev_db_entities::kube_app_catalog::Entity)
            .all(&self.db.conn)
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

    async fn delete_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        // Verify catalog belongs to the organization
        let catalog = lapdev_db_entities::kube_app_catalog::Entity::find_by_id(catalog_id)
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Check for dependencies - kube_environment (any user's environments using this catalog)
        let has_environments = lapdev_db_entities::kube_environment::Entity::find()
            .filter(lapdev_db_entities::kube_environment::Column::AppCatalogId.eq(catalog_id))
            .filter(lapdev_db_entities::kube_environment::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await
            .map_err(ApiError::from)?
            .is_some();

        if has_environments {
            return Err(ApiError::InvalidRequest(
                "Cannot delete app catalog: it has active environments. Please delete them first."
                    .to_string(),
            )
            .into());
        }

        // Soft delete the app catalog
        let mut active_catalog: lapdev_db_entities::kube_app_catalog::ActiveModel = catalog.into();
        active_catalog.deleted_at = ActiveValue::Set(Some(Utc::now().into()));

        active_catalog
            .update(&self.db.conn)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    async fn create_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<(), HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        // Verify app catalog belongs to the organization
        let app_catalog = lapdev_db_entities::kube_app_catalog::Entity::find_by_id(app_catalog_id)
            .filter(lapdev_db_entities::kube_app_catalog::Column::DeletedAt.is_null())
            .one(&self.db.conn)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("App catalog not found".to_string()))?;

        if app_catalog.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Verify cluster belongs to the organization
        let cluster = self
            .db
            .get_kube_cluster(cluster_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Cluster not found".to_string()))?;

        if cluster.organization_id != org_id {
            return Err(ApiError::Unauthorized.into());
        }

        // Create the kube environment
        lapdev_db_entities::kube_environment::ActiveModel {
            id: ActiveValue::Set(Uuid::new_v4()),
            created_at: ActiveValue::Set(Utc::now().into()),
            deleted_at: ActiveValue::Set(None),
            organization_id: ActiveValue::Set(org_id),
            created_by: ActiveValue::Set(user.id),
            user_id: ActiveValue::Set(user.id), // Environment belongs to the user who created it
            app_catalog_id: ActiveValue::Set(app_catalog_id),
            cluster_id: ActiveValue::Set(cluster_id),
            name: ActiveValue::Set(name),
            namespace: ActiveValue::Set(namespace),
            status: ActiveValue::Set(Some("Pending".to_string())),
        }
        .insert(&self.db.conn)
        .await
        .map_err(ApiError::from)?;

        Ok(())
    }
}