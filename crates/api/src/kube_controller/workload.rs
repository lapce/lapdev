use uuid::Uuid;

use lapdev_rpc::error::ApiError;

use super::KubeController;

impl KubeController {
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

            let ports: Vec<lapdev_common::kube::KubeServicePort> =
                serde_json::from_value(updated_db_model.ports.clone()).unwrap_or_default();

            lapdev_common::kube::KubeEnvironmentWorkload {
                id: updated_db_model.id,
                created_at: updated_db_model.created_at,
                environment_id: updated_db_model.environment_id,
                name: updated_db_model.name.clone(),
                namespace: updated_db_model.namespace.clone(),
                kind: updated_db_model.kind.clone(),
                containers,
                ports,
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
                    environment.auth_token.clone(),
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
                    environment.auth_token.clone(),
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
}
