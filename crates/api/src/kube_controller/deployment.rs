use uuid::Uuid;

use lapdev_kube::server::KubeClusterServer;
use lapdev_rpc::error::ApiError;

use super::KubeController;

impl KubeController {
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
