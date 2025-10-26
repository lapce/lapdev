use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use lapdev_common::kube::{
    KubeAppCatalog, KubeAppCatalogWorkload, KubeAppCatalogWorkloadCreate, KubeServiceDetails,
    KubeServiceWithYaml, PagePaginationParams, PaginatedInfo, PaginatedResult,
};
use lapdev_db::api::CachedClusterService;
use lapdev_db_entities::kube_app_catalog_workload_dependency;
use lapdev_kube_rpc::{
    KubeWorkloadYamlOnly, KubeWorkloadsWithResources, NamespacedResourceRequest,
};
use lapdev_rpc::error::ApiError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
use std::collections::{HashMap, HashSet};
use uuid::Uuid;

use crate::kube_controller::resources::clean_service;

use super::{
    resources::{clean_configmap, clean_secret, rebuild_workload_yaml},
    yaml_parser::build_workload_details_from_yaml,
    KubeController,
};
use chrono::Utc;

impl KubeController {
    pub(super) async fn enrich_workloads_with_details(
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

        let raw_workloads = cluster_server
            .rpc_client
            .get_workloads_raw_yaml(tarpc::context::current(), workload_identifiers)
            .await
            .map_err(|e| ApiError::InvalidRequest(format!("Failed to get workload YAML: {}", e)))?
            .map_err(|e| ApiError::InvalidRequest(format!("KubeManager error: {}", e)))?;

        let mut namespaces: HashSet<String> =
            raw_workloads.iter().map(|w| w.namespace.clone()).collect();
        namespaces.extend(workloads.iter().map(|w| w.namespace.clone()));

        let mut services_cache: HashMap<String, Vec<CachedClusterService>> = HashMap::new();
        for namespace in namespaces {
            let services = self
                .db
                .get_active_cluster_services(cluster_id, &namespace)
                .await
                .map_err(ApiError::from)?;
            services_cache.insert(namespace, services);
        }

        let mut results = Vec::with_capacity(raw_workloads.len());
        for raw in raw_workloads {
            let services = services_cache
                .get(&raw.namespace)
                .map(|s| s.as_slice())
                .unwrap_or(&[]);
            match build_workload_details_from_yaml(raw, services) {
                Ok(details) => results.push(details),
                Err(err) => {
                    return Err(ApiError::InvalidRequest(format!(
                        "Failed to process workload YAML: {}",
                        err
                    )))
                }
            }
        }

        Ok(results)
    }

    /// Build KubeWorkloadsWithResources from database-cached catalog data.
    /// This retrieves workload YAML and services from the database cache instead of querying Kubernetes.
    /// Much faster and doesn't require connectivity to the source cluster.
    /// Uses workload label and service selector tables to find only services that select the workloads' pods.
    pub(super) async fn get_catalog_workloads_with_yaml_from_db(
        &self,
        cluster_id: Uuid,
        workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<KubeWorkloadsWithResources, ApiError> {
        // Build workload YAMLs from database
        let mut workload_yamls = Vec::new();
        for workload in &workloads {
            if workload.workload_yaml.trim().is_empty() {
                return Err(ApiError::InvalidRequest(format!(
                    "Workload '{}' has no cached YAML in database",
                    workload.name
                )));
            }
            let raw_yaml = workload.workload_yaml.clone();

            let rebuilt_yaml =
                rebuild_workload_yaml(&workload.kind, &raw_yaml, &workload.containers).map_err(
                    |err| {
                        ApiError::InvalidRequest(format!(
                            "Failed to reconstruct workload YAML for '{}': {}",
                            workload.name, err
                        ))
                    },
                )?;

            let workload_yaml_only = match workload.kind {
                lapdev_common::kube::KubeWorkloadKind::Deployment => {
                    KubeWorkloadYamlOnly::Deployment(rebuilt_yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::StatefulSet => {
                    KubeWorkloadYamlOnly::StatefulSet(rebuilt_yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::DaemonSet => {
                    KubeWorkloadYamlOnly::DaemonSet(rebuilt_yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::ReplicaSet => {
                    KubeWorkloadYamlOnly::ReplicaSet(rebuilt_yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::Pod => {
                    KubeWorkloadYamlOnly::Pod(rebuilt_yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::Job => {
                    KubeWorkloadYamlOnly::Job(rebuilt_yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::CronJob => {
                    KubeWorkloadYamlOnly::CronJob(rebuilt_yaml)
                }
            };

            workload_yamls.push(workload_yaml_only);
        }

        // Get services that select these workloads using the label and selector tables
        let workload_ids: Vec<Uuid> = workloads.iter().map(|w| w.id).collect();
        let cached_services = self
            .db
            .get_services_for_catalog_workloads(cluster_id, &workload_ids)
            .await
            .map_err(ApiError::from)?;

        // Convert CachedClusterService to KubeServiceWithYaml format
        // Note: We need to fetch the service YAML from the database
        let service_names: Vec<String> = cached_services.iter().map(|s| s.name.clone()).collect();
        let service_entities = if !service_names.is_empty() {
            lapdev_db_entities::kube_cluster_service::Entity::find()
                .filter(lapdev_db_entities::kube_cluster_service::Column::ClusterId.eq(cluster_id))
                .filter(lapdev_db_entities::kube_cluster_service::Column::Name.is_in(service_names))
                .filter(lapdev_db_entities::kube_cluster_service::Column::DeletedAt.is_null())
                .all(&self.db.conn)
                .await
                .map_err(ApiError::from)?
        } else {
            Vec::new()
        };

        let mut services_map = HashMap::new();
        for service_entity in service_entities {
            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                serde_json::from_value(service_entity.ports.clone()).unwrap_or_default();
            let selector: std::collections::BTreeMap<String, String> =
                serde_json::from_value(service_entity.selector.clone()).unwrap_or_default();

            let service: Service = serde_yaml::from_str(&service_entity.service_yaml)?;
            let service = clean_service(service);
            let service_yaml = serde_yaml::to_string(&service)?;
            services_map.insert(
                service_entity.name.clone(),
                KubeServiceWithYaml {
                    yaml: service_yaml,
                    details: KubeServiceDetails {
                        name: service_entity.name,
                        ports,
                        selector,
                    },
                },
            );
        }

        let dependency_rows = if workload_ids.is_empty() {
            Vec::new()
        } else {
            kube_app_catalog_workload_dependency::Entity::find()
                .filter(
                    kube_app_catalog_workload_dependency::Column::WorkloadId
                        .is_in(workload_ids.clone()),
                )
                .filter(kube_app_catalog_workload_dependency::Column::DeletedAt.is_null())
                .all(&self.db.conn)
                .await
                .map_err(ApiError::from)?
        };

        let mut dependency_requests: HashMap<String, (HashSet<String>, HashSet<String>)> =
            HashMap::new();

        for row in dependency_rows {
            let namespace = row.namespace.clone();
            let resource_name = row.resource_name.clone();
            let entry = dependency_requests.entry(namespace).or_default();
            match row.resource_type.as_str() {
                "configmap" => {
                    entry.0.insert(resource_name);
                }
                "secret" => {
                    entry.1.insert(resource_name);
                }
                _ => {}
            }
        }

        let mut configmaps_map: HashMap<String, String> = HashMap::new();
        let mut secrets_map: HashMap<String, String> = HashMap::new();

        if !dependency_requests.is_empty() {
            let cluster_server = self
                .get_random_kube_cluster_server(cluster_id)
                .await
                .ok_or_else(|| {
                    ApiError::InvalidRequest(
                        "No connected KubeManager for the app catalog's source cluster".to_string(),
                    )
                })?;

            let requests: Vec<NamespacedResourceRequest> = dependency_requests
                .into_iter()
                .map(
                    |(namespace, (configmaps, secrets))| NamespacedResourceRequest {
                        namespace,
                        configmaps: configmaps.into_iter().collect(),
                        secrets: secrets.into_iter().collect(),
                    },
                )
                .collect();

            if !requests.is_empty() {
                let responses = match cluster_server
                    .rpc_client
                    .get_namespaced_resources(tarpc::context::current(), requests)
                    .await
                {
                    Ok(Ok(res)) => res,
                    Ok(Err(e)) => {
                        return Err(ApiError::InvalidRequest(format!(
                            "Failed to fetch ConfigMaps/Secrets: {e}"
                        )))
                    }
                    Err(e) => {
                        return Err(ApiError::InvalidRequest(format!(
                            "Connection error to source cluster: {e}"
                        )))
                    }
                };

                for response in responses {
                    for (name, yaml) in response.configmaps {
                        let parsed: ConfigMap = serde_yaml::from_str(&yaml).map_err(|err| {
                            ApiError::InvalidRequest(format!(
                                "Failed to parse ConfigMap '{name}' from namespace {}: {err}",
                                response.namespace
                            ))
                        })?;
                        let cleaned = clean_configmap(parsed);
                        let cleaned_yaml = serde_yaml::to_string(&cleaned).map_err(|err| {
                            ApiError::InvalidRequest(format!(
                                "Failed to serialize ConfigMap '{name}' from namespace {}: {err}",
                                response.namespace
                            ))
                        })?;
                        configmaps_map.insert(name, cleaned_yaml);
                    }
                    for (name, yaml) in response.secrets {
                        let parsed: Secret = serde_yaml::from_str(&yaml).map_err(|err| {
                            ApiError::InvalidRequest(format!(
                                "Failed to parse Secret '{name}' from namespace {}: {err}",
                                response.namespace
                            ))
                        })?;
                        let cleaned = clean_secret(parsed);
                        let cleaned_yaml = serde_yaml::to_string(&cleaned).map_err(|err| {
                            ApiError::InvalidRequest(format!(
                                "Failed to serialize Secret '{name}' from namespace {}: {err}",
                                response.namespace
                            ))
                        })?;
                        secrets_map.insert(name, cleaned_yaml);
                    }
                }
            }
        }

        tracing::info!(
            "Built catalog workload resources from DB: {} workloads, {} services, {} configmaps, {} secrets (cluster: {})",
            workload_yamls.len(),
            services_map.len(),
            configmaps_map.len(),
            secrets_map.len(),
            cluster_id,
        );

        Ok(KubeWorkloadsWithResources {
            workloads: workload_yamls,
            services: services_map,
            configmaps: configmaps_map,
            secrets: secrets_map,
        })
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
                    last_sync_actor_id: catalog.last_sync_actor_id,
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
            last_sync_actor_id: catalog.last_sync_actor_id,
        })
    }

    pub async fn get_app_catalog_workloads(
        &self,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<Vec<KubeAppCatalogWorkload>, ApiError> {
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
            .map_err(ApiError::from)?;

        self.refresh_cluster_namespace_watches(catalog.cluster_id)
            .await;

        Ok(())
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
            .map_err(ApiError::from)?;

        let _ = self
            .db
            .bump_app_catalog_sync_version(workload.app_catalog_id, Utc::now().into())
            .await
            .map_err(ApiError::from)?;

        self.refresh_cluster_namespace_watches(workload.cluster_id)
            .await;

        Ok(())
    }

    pub async fn update_app_catalog_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
    ) -> Result<(), ApiError> {
        // Validate containers
        super::validation::validate_containers(&containers)?;

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
            .map_err(ApiError::from)?;

        let new_version = self
            .db
            .bump_app_catalog_sync_version(catalog.id, Utc::now().into())
            .await
            .map_err(ApiError::from)?;

        self.db
            .update_catalog_workload_versions(&[workload.id], new_version)
            .await
            .map_err(ApiError::from)
    }

    pub async fn add_workloads_to_app_catalog(
        &self,
        org_id: Uuid,
        _user_id: Uuid,
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

        let inserted_ids = match self
            .db
            .insert_enriched_workloads_to_catalog(
                &txn,
                catalog_id,
                catalog.cluster_id,
                enriched_workloads,
                now,
            )
            .await
        {
            Ok(ids) => ids,
            Err(db_err) => {
                txn.rollback().await.map_err(ApiError::from)?;
                if matches!(
                    db_err.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    return Err(ApiError::InvalidRequest(
                        "One or more selected workloads already exist in this catalog".to_string(),
                    ));
                } else {
                    return Err(ApiError::from(anyhow::Error::from(db_err)));
                }
            }
        };

        txn.commit().await.map_err(ApiError::from)?;

        let new_version = self
            .db
            .bump_app_catalog_sync_version(catalog.id, Utc::now().into())
            .await
            .map_err(ApiError::from)?;

        self.db
            .update_catalog_workload_versions(&inserted_ids, new_version)
            .await
            .map_err(ApiError::from)?;

        self.refresh_cluster_namespace_watches(catalog.cluster_id)
            .await;

        Ok(())
    }
}
