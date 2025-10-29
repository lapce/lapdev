use uuid::Uuid;

use lapdev_common::{kube::KubeEnvironmentPreviewUrl, utils::rand_string, LAPDEV_API_HOST};
use lapdev_rpc::error::ApiError;

use super::KubeController;

impl KubeController {
    pub async fn create_environment_preview_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
        request: lapdev_common::kube::CreateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<KubeEnvironmentPreviewUrl, ApiError> {
        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Verify service exists and belongs to the environment
        let service = self
            .db
            .get_kube_environment_service_by_id(request.service_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Service not found".to_string()))?;

        // Verify service belongs to the environment
        if service.environment_id != environment_id {
            return Err(ApiError::InvalidRequest(
                "Service not found in environment".to_string(),
            ));
        }

        // Validate the port exists on the service
        let port_exists = service.ports.iter().any(|p| {
            p.port == request.port
                && (request.port_name.is_none() || p.name.as_ref() == request.port_name.as_ref())
        });

        if !port_exists {
            return Err(ApiError::InvalidRequest(
                "Specified port does not exist on the service".to_string(),
            ));
        }

        // Auto-generate name based on service and port
        let auto_name = format!("{}-{}-{}", request.port, service.name, rand_string(12));

        // Set defaults
        let protocol = request.protocol.unwrap_or_else(|| "HTTP".to_string());
        let access_level = request
            .access_level
            .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Organization);

        let url = format!("https://{auto_name}.{}", LAPDEV_API_HOST);

        // Create preview URL in database
        let preview_url = match self
            .db
            .create_environment_preview_url(
                environment_id,
                request.service_id,
                user_id,
                auto_name,
                request.description,
                request.port,
                request.port_name,
                protocol.clone(),
                access_level.clone(),
            )
            .await
        {
            Ok(url) => url,
            Err(db_err) => {
                // Check if this is a unique constraint violation
                if matches!(
                    db_err.sql_err(),
                    Some(sea_orm::SqlErr::UniqueConstraintViolation(_))
                ) {
                    return Err(ApiError::InvalidRequest(
                        "A preview URL already exists for this service and port combination"
                            .to_string(),
                    ));
                } else {
                    return Err(ApiError::from(anyhow::Error::from(db_err)));
                }
            }
        };

        Ok(KubeEnvironmentPreviewUrl {
            id: preview_url.id,
            created_at: preview_url.created_at,
            environment_id: preview_url.environment_id,
            service_id: preview_url.service_id,
            name: preview_url.name,
            description: preview_url.description,
            port: preview_url.port,
            port_name: preview_url.port_name,
            protocol: preview_url.protocol,
            access_level: preview_url
                .access_level
                .parse()
                .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Organization),
            created_by: preview_url.created_by,
            last_accessed_at: preview_url.last_accessed_at,
            url,
        })
    }

    pub async fn get_environment_preview_urls(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentPreviewUrl>, ApiError> {
        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        let preview_urls = self
            .db
            .get_environment_preview_urls(environment_id)
            .await
            .map_err(ApiError::from)?;

        Ok(preview_urls
            .into_iter()
            .map(|preview_url| {
                let url = format!("https://{}.{}", preview_url.name, LAPDEV_API_HOST);

                KubeEnvironmentPreviewUrl {
                    id: preview_url.id,
                    created_at: preview_url.created_at,
                    environment_id: preview_url.environment_id,
                    service_id: preview_url.service_id,
                    name: preview_url.name,
                    description: preview_url.description,
                    port: preview_url.port,
                    port_name: preview_url.port_name,
                    protocol: preview_url.protocol,
                    access_level: preview_url
                        .access_level
                        .parse()
                        .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Organization),
                    created_by: preview_url.created_by,
                    last_accessed_at: preview_url.last_accessed_at,
                    url,
                }
            })
            .collect())
    }

    pub async fn update_environment_preview_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        preview_url_id: Uuid,
        request: lapdev_common::kube::UpdateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<KubeEnvironmentPreviewUrl, ApiError> {
        // Get the preview URL and verify ownership
        let preview_url = self
            .db
            .get_environment_preview_url(preview_url_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Preview URL not found".to_string()))?;

        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(preview_url.environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Update the preview URL
        let updated_preview_url = self
            .db
            .update_environment_preview_url(
                preview_url_id,
                request.description,
                request.access_level,
            )
            .await
            .map_err(ApiError::from)?;

        let url = format!("https://{}.{}", updated_preview_url.name, LAPDEV_API_HOST);

        Ok(KubeEnvironmentPreviewUrl {
            id: updated_preview_url.id,
            created_at: updated_preview_url.created_at,
            environment_id: updated_preview_url.environment_id,
            service_id: updated_preview_url.service_id,
            name: updated_preview_url.name,
            description: updated_preview_url.description,
            port: updated_preview_url.port,
            port_name: updated_preview_url.port_name,
            protocol: updated_preview_url.protocol,
            access_level: updated_preview_url
                .access_level
                .parse()
                .unwrap_or(lapdev_common::kube::PreviewUrlAccessLevel::Organization),
            created_by: updated_preview_url.created_by,
            last_accessed_at: updated_preview_url.last_accessed_at,
            url,
        })
    }

    pub async fn delete_environment_preview_url(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        preview_url_id: Uuid,
    ) -> Result<(), ApiError> {
        // Get the preview URL and verify ownership
        let preview_url = self
            .db
            .get_environment_preview_url(preview_url_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Preview URL not found".to_string()))?;

        // Verify environment belongs to the organization and check ownership
        let environment = self
            .db
            .get_kube_environment(preview_url.environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        self.db
            .delete_environment_preview_url(preview_url_id)
            .await
            .map_err(ApiError::from)
    }
}
