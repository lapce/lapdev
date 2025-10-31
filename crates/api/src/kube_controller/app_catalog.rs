use k8s_openapi::api::core::v1::{ConfigMap, Secret, Service};
use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
use lapdev_common::kube::{
    KubeAppCatalog, KubeAppCatalogWorkload, KubeAppCatalogWorkloadCreate, KubeServiceDetails,
    KubeServiceWithYaml, KubeWorkloadDetails, PagePaginationParams, PaginatedInfo, PaginatedResult,
    ProxyPortRoute, DEFAULT_SIDECAR_PROXY_METRICS_PORT, DEFAULT_SIDECAR_PROXY_PORT,
    SIDECAR_PROXY_DYNAMIC_PORT_START,
};
use lapdev_db::api::CachedClusterService;
use lapdev_db_entities::kube_app_catalog_workload_dependency;
use lapdev_kube_rpc::{
    KubeWorkloadYamlOnly, KubeWorkloadsWithResources, NamespacedResourceRequest,
    NamespacedResourceResponse,
};
use lapdev_rpc::error::ApiError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, TransactionTrait};
use std::collections::{HashMap, HashSet};
use std::convert::TryFrom;
use uuid::Uuid;

use crate::kube_controller::resources::clean_service;

use super::{
    resources::{clean_configmap, clean_secret, rebuild_workload_yaml, SidecarInjectionOptions},
    yaml_parser::build_workload_details_from_yaml,
    KubeController,
};
use chrono::Utc;

pub struct SidecarInjectionContext<'a> {
    pub environment_id: Uuid,
    pub namespace: &'a str,
    pub auth_token: &'a str,
    pub manager_namespace: Option<&'a str>,
}

struct PreparedWorkloads {
    workload_yamls: Vec<KubeWorkloadYamlOnly>,
    proxy_routes: HashMap<Uuid, Vec<ProxyPortRoute>>,
    workloads_for_details: Vec<KubeAppCatalogWorkload>,
}

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
        injection_ctx: &SidecarInjectionContext<'_>,
    ) -> Result<(KubeWorkloadsWithResources, Vec<KubeWorkloadDetails>), ApiError> {
        let workload_ids: Vec<Uuid> = workloads.iter().map(|w| w.id).collect();
        let services_by_workload = self
            .load_services_for_catalog_workloads(cluster_id, &workload_ids)
            .await?;

        let PreparedWorkloads {
            workload_yamls,
            proxy_routes,
            workloads_for_details,
        } = self.prepare_workloads_from_cache(&workloads, &services_by_workload, injection_ctx)?;

        let services_map =
            self.prepare_services_from_cache(&services_by_workload, &proxy_routes)?;

        let (configmaps_map, secrets_map) = self
            .fetch_dependency_resources(cluster_id, &workload_ids)
            .await?;

        tracing::info!(
            "Built catalog workload resources from DB: {} workloads, {} services, {} configmaps, {} secrets (cluster: {})",
            workload_yamls.len(),
            services_map.len(),
            configmaps_map.len(),
            secrets_map.len(),
            cluster_id,
        );

        let workload_details = Self::prepare_workload_details_from_catalog(
            workloads_for_details,
            injection_ctx.namespace,
        )?;

        Ok((
            KubeWorkloadsWithResources {
                workloads: workload_yamls,
                services: services_map,
                configmaps: configmaps_map,
                secrets: secrets_map,
            },
            workload_details,
        ))
    }

    async fn load_services_for_catalog_workloads(
        &self,
        cluster_id: Uuid,
        workload_ids: &[Uuid],
    ) -> Result<HashMap<Uuid, Vec<CachedClusterService>>, ApiError> {
        self.db
            .get_services_for_catalog_workloads(cluster_id, workload_ids)
            .await
            .map_err(ApiError::from)
    }

    fn prepare_workloads_from_cache(
        &self,
        workloads: &[KubeAppCatalogWorkload],
        services_by_workload: &HashMap<Uuid, Vec<CachedClusterService>>,
        injection_ctx: &SidecarInjectionContext<'_>,
    ) -> Result<PreparedWorkloads, ApiError> {
        let mut workload_yamls = Vec::with_capacity(workloads.len());
        let mut proxy_routes: HashMap<Uuid, Vec<ProxyPortRoute>> = HashMap::new();
        let mut workloads_for_details = Vec::with_capacity(workloads.len());

        for workload in workloads {
            if workload.workload_yaml.trim().is_empty() {
                return Err(ApiError::InvalidRequest(format!(
                    "Workload '{}' has no cached YAML in database",
                    workload.name
                )));
            }

            let routes = Self::build_proxy_routes(
                services_by_workload
                    .get(&workload.id)
                    .map(|services| services.as_slice()),
            );

            let raw_yaml = workload.workload_yaml.clone();
            let sidecar_options = SidecarInjectionOptions {
                environment_id: injection_ctx.environment_id,
                workload_id: workload.id,
                namespace: injection_ctx.namespace,
                auth_token: injection_ctx.auth_token,
                manager_namespace: injection_ctx.manager_namespace,
                proxy_routes: routes.as_slice(),
            };

            let rebuilt_yaml = rebuild_workload_yaml(
                &workload.kind,
                &raw_yaml,
                &workload.containers,
                Some(&sidecar_options),
            )
            .map_err(|err| {
                ApiError::InvalidRequest(format!(
                    "Failed to reconstruct workload YAML for '{}': {}",
                    workload.name, err
                ))
            })?;

            let mut workload_for_details = workload.clone();
            workload_for_details.workload_yaml = rebuilt_yaml.clone();
            workloads_for_details.push(workload_for_details);

            let workload_yaml_only = Self::to_workload_yaml_only(&workload.kind, rebuilt_yaml);
            workload_yamls.push(workload_yaml_only);

            if !routes.is_empty() {
                proxy_routes.insert(workload.id, routes);
            }
        }

        Ok(PreparedWorkloads {
            workload_yamls,
            proxy_routes,
            workloads_for_details,
        })
    }

    fn prepare_services_from_cache(
        &self,
        services_by_workload: &HashMap<Uuid, Vec<CachedClusterService>>,
        proxy_routes: &HashMap<Uuid, Vec<ProxyPortRoute>>,
    ) -> Result<HashMap<String, KubeServiceWithYaml>, ApiError> {
        let mut services_map = HashMap::new();

        for services in services_by_workload.values() {
            for service in services {
                if services_map.contains_key(&service.name) {
                    continue;
                }

                let ports = service.ports.clone();
                let selector = service.selector.clone();
                let parsed: Service =
                    serde_yaml::from_str(&service.service_yaml).map_err(|err| {
                        ApiError::InvalidRequest(format!(
                            "Failed to parse cached Service '{}' YAML: {}",
                            service.name, err
                        ))
                    })?;
                let cleaned = clean_service(parsed);
                let cleaned_yaml = serde_yaml::to_string(&cleaned).map_err(|err| {
                    ApiError::InvalidRequest(format!(
                        "Failed to serialize cleaned Service '{}' YAML: {}",
                        service.name, err
                    ))
                })?;

                services_map.insert(
                    service.name.clone(),
                    KubeServiceWithYaml {
                        yaml: cleaned_yaml,
                        details: KubeServiceDetails {
                            name: service.name.clone(),
                            ports,
                            selector,
                        },
                    },
                );
            }
        }

        for (workload_id, routes) in proxy_routes {
            if let Some(services) = services_by_workload.get(workload_id) {
                for service in services {
                    if let Some(entry) = services_map.get_mut(&service.name) {
                        rewrite_service_for_sidecar(entry, routes)?;
                    }
                }
            }
        }

        Ok(services_map)
    }

    async fn fetch_dependency_resources(
        &self,
        cluster_id: Uuid,
        workload_ids: &[Uuid],
    ) -> Result<(HashMap<String, String>, HashMap<String, String>), ApiError> {
        if workload_ids.is_empty() {
            return Ok((HashMap::new(), HashMap::new()));
        }

        let dependency_rows = kube_app_catalog_workload_dependency::Entity::find()
            .filter(
                kube_app_catalog_workload_dependency::Column::WorkloadId
                    .is_in(workload_ids.to_vec()),
            )
            .filter(kube_app_catalog_workload_dependency::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await
            .map_err(ApiError::from)?;

        let mut dependency_requests: HashMap<String, (HashSet<String>, HashSet<String>)> =
            HashMap::new();

        for row in dependency_rows {
            let entry = dependency_requests
                .entry(row.namespace.clone())
                .or_default();
            match row.resource_type.as_str() {
                "configmap" => {
                    entry.0.insert(row.resource_name.clone());
                }
                "secret" => {
                    entry.1.insert(row.resource_name.clone());
                }
                _ => {}
            }
        }

        if dependency_requests.is_empty() {
            return Ok((HashMap::new(), HashMap::new()));
        }

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

        if requests.is_empty() {
            return Ok((HashMap::new(), HashMap::new()));
        }

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

        Self::extract_dependency_resources(responses)
    }

    fn build_proxy_routes(services: Option<&[CachedClusterService]>) -> Vec<ProxyPortRoute> {
        let mut routes = Vec::new();

        if let Some(services) = services {
            let mut assigned_ports: HashMap<(u16, u16), u16> = HashMap::new();
            let mut used_proxy_ports: HashSet<u16> = HashSet::new();
            used_proxy_ports.insert(DEFAULT_SIDECAR_PROXY_PORT);
            used_proxy_ports.insert(DEFAULT_SIDECAR_PROXY_METRICS_PORT);
            let mut next_proxy_port = u32::from(SIDECAR_PROXY_DYNAMIC_PORT_START);

            for service in services {
                for port in &service.ports {
                    let service_port = match u16::try_from(port.port) {
                        Ok(port) if port > 0 => port,
                        _ => continue,
                    };

                    let raw_target = port.original_target_port.or(port.target_port);
                    let target_port = match raw_target {
                        Some(value) => match u16::try_from(value) {
                            Ok(tp) if tp > 0 => tp,
                            _ => continue,
                        },
                        None => {
                            // TODO: support named targetPort values when the service uses port names.
                            continue;
                        }
                    };

                    let key = (service_port, target_port);
                    let proxy_port = if let Some(existing) = assigned_ports.get(&key) {
                        *existing
                    } else {
                        let allocated = loop {
                            if next_proxy_port > u32::from(u16::MAX) {
                                tracing::warn!(
                                    service_port,
                                    target_port,
                                    "exhausted dynamic proxy port range; reusing service port for proxy"
                                );
                                break service_port;
                            }

                            let candidate = next_proxy_port as u16;
                            next_proxy_port += 1;

                            if candidate == service_port || candidate == target_port {
                                continue;
                            }

                            if !used_proxy_ports.insert(candidate) {
                                continue;
                            }

                            break candidate;
                        };

                        assigned_ports.insert(key, allocated);
                        allocated
                    };

                    routes.push(ProxyPortRoute {
                        proxy_port,
                        service_port,
                        target_port,
                    });
                }
            }
        }

        routes
    }

    fn to_workload_yaml_only(
        kind: &lapdev_common::kube::KubeWorkloadKind,
        yaml: String,
    ) -> KubeWorkloadYamlOnly {
        match kind {
            lapdev_common::kube::KubeWorkloadKind::Deployment => {
                KubeWorkloadYamlOnly::Deployment(yaml)
            }
            lapdev_common::kube::KubeWorkloadKind::StatefulSet => {
                KubeWorkloadYamlOnly::StatefulSet(yaml)
            }
            lapdev_common::kube::KubeWorkloadKind::DaemonSet => {
                KubeWorkloadYamlOnly::DaemonSet(yaml)
            }
            lapdev_common::kube::KubeWorkloadKind::ReplicaSet => {
                KubeWorkloadYamlOnly::ReplicaSet(yaml)
            }
            lapdev_common::kube::KubeWorkloadKind::Pod => KubeWorkloadYamlOnly::Pod(yaml),
            lapdev_common::kube::KubeWorkloadKind::Job => KubeWorkloadYamlOnly::Job(yaml),
            lapdev_common::kube::KubeWorkloadKind::CronJob => KubeWorkloadYamlOnly::CronJob(yaml),
        }
    }

    fn extract_dependency_resources(
        responses: Vec<NamespacedResourceResponse>,
    ) -> Result<(HashMap<String, String>, HashMap<String, String>), ApiError> {
        let mut configmaps_map: HashMap<String, String> = HashMap::new();
        let mut secrets_map: HashMap<String, String> = HashMap::new();

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

        Ok((configmaps_map, secrets_map))
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

        let catalog_id = self
            .db
            .create_app_catalog_with_enriched_workloads(
                org_id,
                user_id,
                cluster_id,
                name,
                description,
                enriched_workloads,
            )
            .await
            .map_err(ApiError::from)?;

        self.refresh_cluster_namespace_watches(cluster_id).await;

        Ok(catalog_id)
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

fn rewrite_service_for_sidecar(
    service_entry: &mut KubeServiceWithYaml,
    routes: &[ProxyPortRoute],
) -> Result<(), ApiError> {
    if routes.is_empty() {
        return Ok(());
    }

    let mut service: Service = serde_yaml::from_str(&service_entry.yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse cached Service '{}' YAML: {}",
            service_entry.details.name, err
        ))
    })?;

    if let Some(spec) = service.spec.as_mut() {
        if let Some(ports) = spec.ports.as_mut() {
            for port in ports.iter_mut() {
                let service_port = port.port;
                if let Some(route) = routes
                    .iter()
                    .find(|route| route.service_port as i32 == service_port)
                {
                    port.target_port = Some(IntOrString::Int(route.proxy_port as i32));
                }
            }
        }
    }

    service_entry.yaml = serde_yaml::to_string(&service).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize Service '{}' YAML after sidecar rewrite: {}",
            service_entry.details.name, err
        ))
    })?;

    for port in service_entry.details.ports.iter_mut() {
        if let Some(route) = routes
            .iter()
            .find(|route| route.service_port as i32 == port.port)
        {
            if port.original_target_port.is_none() {
                port.original_target_port = Some(route.target_port as i32);
            }
            port.target_port = Some(route.proxy_port as i32);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::core::v1::Service;
    use k8s_openapi::apimachinery::pkg::util::intstr::IntOrString;
    use lapdev_common::kube::KubeServicePort;
    use std::collections::BTreeMap;

    #[test]
    fn rewrite_service_preserves_original_target_port() {
        let initial_yaml = r#"
apiVersion: v1
kind: Service
metadata:
  name: test
spec:
  ports:
    - port: 80
      targetPort: 8080
      protocol: TCP
  selector:
    app: demo
"#
        .to_string();

        let mut service_entry = KubeServiceWithYaml {
            yaml: initial_yaml,
            details: KubeServiceDetails {
                name: "test".to_string(),
                ports: vec![KubeServicePort {
                    name: None,
                    port: 80,
                    target_port: Some(8080),
                    protocol: Some("TCP".to_string()),
                    node_port: None,
                    original_target_port: None,
                }],
                selector: BTreeMap::new(),
            },
        };

        let routes = vec![ProxyPortRoute {
            proxy_port: DEFAULT_SIDECAR_PROXY_PORT,
            service_port: 80,
            target_port: 8080,
        }];

        rewrite_service_for_sidecar(&mut service_entry, &routes).unwrap();

        let updated: Service = serde_yaml::from_str(&service_entry.yaml).unwrap();
        let rewritten_target = updated
            .spec
            .as_ref()
            .and_then(|spec| spec.ports.as_ref())
            .and_then(|ports| ports.first())
            .and_then(|port| port.target_port.clone())
            .expect("target port should be set");

        let target_value = match rewritten_target {
            IntOrString::Int(value) => value,
            IntOrString::String(_) => panic!("expected numeric target port"),
        };

        assert_eq!(target_value, DEFAULT_SIDECAR_PROXY_PORT as i32);

        let details_port = &service_entry.details.ports[0];
        assert_eq!(
            details_port.target_port,
            Some(DEFAULT_SIDECAR_PROXY_PORT as i32)
        );
        assert_eq!(details_port.original_target_port, Some(8080));
    }
}
