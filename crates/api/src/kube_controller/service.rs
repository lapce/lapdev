use uuid::Uuid;

use lapdev_rpc::error::ApiError;

use super::KubeController;

impl KubeController {
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

        if let Some(base_environment_id) = environment.base_environment_id {
            // Branch environments inherit service definitions from their base environment.
            // We return the base services but rewrite the environment_id so downstream callers
            // keep treating them as belonging to the branch (important for preview URLs).
            let base_environment = self
                .db
                .get_kube_environment(base_environment_id)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| ApiError::InvalidRequest("Base environment not found".to_string()))?;

            if base_environment.organization_id != org_id {
                return Err(ApiError::Unauthorized);
            }

            let mut base_services = self
                .db
                .get_environment_services(base_environment_id)
                .await
                .map_err(ApiError::from)?;
            for service in &mut base_services {
                service.environment_id = environment.id;
            }

            return Ok(base_services);
        }

        let services = self
            .db
            .get_environment_services(environment_id)
            .await
            .map_err(ApiError::from)?;

        Ok(services)
    }
}
