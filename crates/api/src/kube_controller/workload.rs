use uuid::Uuid;

use std::collections::HashMap;

use lapdev_common::kube::{KubeAppCatalogWorkload, KubeServiceWithYaml};
use lapdev_rpc::error::ApiError;

use super::{
    resources::{rename_service_yaml, rename_workload_yaml},
    KubeController,
};

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
                catalog_sync_version: updated_db_model.catalog_sync_version,
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

        if environment.base_environment_id.is_some() {
            self.deploy_branch_workload(
                &cluster_server,
                &environment,
                &updated_workload.name,
                updated_workload.containers,
                environment_labels,
            )
            .await
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

    async fn deploy_branch_workload(
        &self,
        cluster_server: &lapdev_kube::server::KubeClusterServer,
        environment: &lapdev_db_entities::kube_environment::Model,
        base_workload_name: &str,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        mut environment_labels: HashMap<String, String>,
    ) -> Result<(), ApiError> {
        tracing::info!(
            "Creating branch deployment for workload '{}' in branch environment '{}' (namespace '{}')",
            base_workload_name,
            environment.name,
            environment.namespace
        );

        let app_workloads = self
            .db
            .get_app_catalog_workloads(environment.app_catalog_id)
            .await
            .map_err(ApiError::from)?;

        let base_catalog_workload = app_workloads
            .into_iter()
            .find(|w| w.name == base_workload_name)
            .ok_or_else(|| {
                ApiError::InvalidRequest(format!(
                    "Base workload '{}' not found in app catalog",
                    base_workload_name
                ))
            })?;

        let branch_workload = KubeAppCatalogWorkload {
            containers,
            ..base_catalog_workload
        };

        let mut workloads_with_resources = self
            .get_catalog_workloads_with_yaml_from_db(
                environment.cluster_id,
                vec![branch_workload.clone()],
            )
            .await?;

        let branch_deployment_name = format!("{}-{}", base_workload_name, environment.id);
        for workload_yaml in &mut workloads_with_resources.workloads {
            rename_workload_yaml(workload_yaml, &branch_deployment_name)
                .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;
        }

        let mut updated_services = HashMap::new();
        for (service_name, service_with_yaml) in workloads_with_resources.services.into_iter() {
            let branch_service_name = format!("{}-{}", service_name, environment.id);
            let updated_yaml = rename_service_yaml(
                &service_with_yaml.yaml,
                &branch_service_name,
                &branch_deployment_name,
            )
            .map_err(|e| ApiError::InvalidRequest(e.to_string()))?;

            let mut details = service_with_yaml.details.clone();
            details.name = branch_service_name.clone();

            updated_services.insert(
                branch_service_name,
                KubeServiceWithYaml {
                    yaml: updated_yaml,
                    details,
                },
            );
        }
        workloads_with_resources.services = updated_services;
        workloads_with_resources.configmaps.clear();
        workloads_with_resources.secrets.clear();

        environment_labels.insert(
            "lapdev.branch-environment".to_string(),
            environment.id.to_string(),
        );
        environment_labels.insert(
            "lapdev.base-workload".to_string(),
            base_workload_name.to_string(),
        );
        environment_labels.insert(
            "lapdev.io/branch-environment-id".to_string(),
            environment.id.to_string(),
        );
        environment_labels.insert(
            "lapdev.io/routing-key".to_string(),
            format!("branch-{}", environment.id),
        );
        environment_labels.insert(
            "lapdev.io/proxy-target-port".to_string(),
            "8080".to_string(),
        );

        self.deploy_environment_resources(
            cluster_server,
            &environment.namespace,
            &environment.name,
            environment.id,
            Some(environment.auth_token.clone()),
            workloads_with_resources,
        )
        .await?;

        if let Err(err) = cluster_server
            .rpc_client
            .refresh_branch_service_routes(tarpc::context::current(), environment.id)
            .await
        {
            tracing::warn!(
                "Failed to refresh branch service routes for environment {}: {}",
                environment.id,
                err
            );
        }

        Ok(())
    }
}
