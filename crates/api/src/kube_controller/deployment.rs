use std::collections::HashMap;

use lapdev_kube::server::KubeClusterServer;
use lapdev_rpc::error::ApiError;

use super::KubeController;

impl KubeController {
    pub(super) async fn deploy_environment_resources(
        &self,
        target_server: &KubeClusterServer,
        environment: &lapdev_db_entities::kube_environment::Model,
        workloads_with_resources: lapdev_kube_rpc::KubeWorkloadsWithResources,
        extra_labels: Option<HashMap<String, String>>,
    ) -> Result<(), ApiError> {
        let namespace = &environment.namespace;
        let environment_name = &environment.name;

        tracing::info!(
            "Deploying environment resources for '{}' in namespace '{}'",
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
        if environment.base_environment_id.is_some() {
            environment_labels.insert(
                "lapdev.io/branch-environment-id".to_string(),
                environment.id.to_string(),
            );
        }
        if let Some(extra) = extra_labels {
            for (key, value) in extra {
                environment_labels.insert(key, value);
            }
        }

        // Deploy all workloads and resources in a single call
        if environment.auth_token.is_empty() {
            return Err(ApiError::InvalidRequest(
                "Environment auth token is required".to_string(),
            ));
        }
        let auth_token = environment.auth_token.clone();

        match target_server
            .rpc_client
            .deploy_workload_yaml(
                tarpc::context::current(),
                environment.id,
                auth_token,
                namespace.to_string(),
                workloads_with_resources,
                environment_labels,
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
