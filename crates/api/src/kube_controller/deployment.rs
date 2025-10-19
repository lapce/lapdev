use std::collections::HashMap;
use uuid::Uuid;

use lapdev_common::kube::{KubeAppCatalogWorkload, KubeServiceDetails, KubeServiceWithYaml};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::{KubeWorkloadYamlOnly, KubeWorkloadsWithResources};
use lapdev_rpc::error::ApiError;
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter};

use super::KubeController;

impl KubeController {
    /// Build KubeWorkloadsWithResources from database-cached data instead of querying Kubernetes.
    /// This is much faster and doesn't require connectivity to the source cluster.
    pub(super) async fn get_workloads_yaml_from_db(
        &self,
        cluster_id: Uuid,
        namespace: &str,
        workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<KubeWorkloadsWithResources, ApiError> {
        // Build workload YAMLs from database
        let mut workload_yamls = Vec::new();
        for workload in &workloads {
            let yaml = workload.workload_yaml.clone().ok_or_else(|| {
                ApiError::InvalidRequest(format!(
                    "Workload '{}' has no cached YAML in database",
                    workload.name
                ))
            })?;

            let workload_yaml_only = match workload.kind {
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
                lapdev_common::kube::KubeWorkloadKind::Pod => {
                    KubeWorkloadYamlOnly::Pod(yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::Job => {
                    KubeWorkloadYamlOnly::Job(yaml)
                }
                lapdev_common::kube::KubeWorkloadKind::CronJob => {
                    KubeWorkloadYamlOnly::CronJob(yaml)
                }
            };

            workload_yamls.push(workload_yaml_only);
        }

        // Get services from database cache
        let services = lapdev_db_entities::kube_cluster_service::Entity::find()
            .filter(lapdev_db_entities::kube_cluster_service::Column::ClusterId.eq(cluster_id))
            .filter(lapdev_db_entities::kube_cluster_service::Column::Namespace.eq(namespace))
            .filter(lapdev_db_entities::kube_cluster_service::Column::DeletedAt.is_null())
            .all(&self.db.conn)
            .await
            .map_err(ApiError::from)?;

        let mut services_map = HashMap::new();
        for service in services {
            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                serde_json::from_value(service.ports.clone()).unwrap_or_default();
            let selector_btree: std::collections::BTreeMap<String, String> =
                serde_json::from_value(service.selector.clone()).unwrap_or_default();
            let selector: HashMap<String, String> = selector_btree.into_iter().collect();

            services_map.insert(
                service.name.clone(),
                KubeServiceWithYaml {
                    yaml: service.service_yaml,
                    details: KubeServiceDetails {
                        name: service.name,
                        ports,
                        selector,
                    },
                },
            );
        }

        tracing::info!(
            "Built workload resources from DB: {} workloads, {} services (cluster: {}, namespace: {})",
            workload_yamls.len(),
            services_map.len(),
            cluster_id,
            namespace
        );

        Ok(KubeWorkloadsWithResources {
            workloads: workload_yamls,
            services: services_map,
            // ConfigMaps and Secrets aren't cached in DB yet, but the workload YAML
            // should already reference them, so they'll be deployed as part of the workload
            configmaps: HashMap::new(),
            secrets: HashMap::new(),
        })
    }

    /// Legacy method that queries Kubernetes via RPC for workload YAML.
    /// Consider using get_workloads_yaml_from_db instead for better performance.
    pub(super) async fn get_workloads_yaml_for_catalog(
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

    pub(super) async fn deploy_app_catalog_with_yaml(
        &self,
        target_server: &KubeClusterServer,
        namespace: &str,
        environment_name: &str,
        environment_id: Uuid,
        environment_auth_token: Option<String>,
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
        let auth_token = environment_auth_token.ok_or_else(|| {
            ApiError::InvalidRequest("Environment auth token is required".to_string())
        })?;

        match target_server
            .rpc_client
            .deploy_workload_yaml(
                tarpc::context::current(),
                environment_id,
                auth_token,
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
}
