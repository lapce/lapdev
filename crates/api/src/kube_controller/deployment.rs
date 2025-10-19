use uuid::Uuid;

use lapdev_common::kube::KubeAppCatalogWorkload;
use lapdev_kube::server::KubeClusterServer;
use lapdev_rpc::error::ApiError;

use super::KubeController;

impl KubeController {
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
