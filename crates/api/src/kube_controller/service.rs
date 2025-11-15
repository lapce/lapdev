use uuid::Uuid;

use lapdev_rpc::error::ApiError;
use std::collections::HashSet;

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

        let mut services = self
            .db
            .get_environment_services(environment_id)
            .await
            .map_err(ApiError::from)?;

        if let Some(base_environment_id) = environment.base_environment_id {
            let base_services = self
                .db
                .get_environment_services(base_environment_id)
                .await
                .map_err(ApiError::from)?;
            let mut existing: HashSet<String> = HashSet::new();
            for service in &services {
                existing.insert(service.name.clone());
                existing.insert(normalize_service_name(&service.name, environment.id));
            }

            for mut base_service in base_services {
                if existing.contains(&base_service.name) {
                    continue;
                }
                base_service.environment_id = environment.id;
                services.push(base_service);
            }
        }

        Ok(services)
    }
}

fn normalize_service_name(name: &str, branch_environment_id: Uuid) -> String {
    let suffix = format!("-{}", branch_environment_id);
    if name.ends_with(&suffix) {
        name[..name.len() - suffix.len()].to_string()
    } else {
        name.to_string()
    }
}
