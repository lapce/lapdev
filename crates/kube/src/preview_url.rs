use anyhow::Result;
use lapdev_common::kube::PreviewUrlAccessLevel;
use lapdev_db::api::DbApi;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone)]
pub struct PreviewUrlInfo {
    pub service_name: String,
    pub port: u16,
    pub environment_hash: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PreviewUrlTarget {
    pub cluster_id: Uuid,
    pub namespace: String,
    pub service_name: String,
    pub service_port: u16,
    pub access_level: PreviewUrlAccessLevel,
    pub protocol: String,
    pub environment_id: Uuid,
    pub organization_id: Uuid,
    pub preview_url_id: Uuid,
}

#[derive(Debug, thiserror::Error)]
pub enum PreviewUrlError {
    #[error("Invalid preview URL format")]
    InvalidFormat,
    #[error("Invalid port number")]
    InvalidPort,
    #[error("Environment not found")]
    EnvironmentNotFound,
    #[error("Service not found")]
    ServiceNotFound,
    #[error("Preview URL not configured")]
    PreviewUrlNotConfigured,
    #[error("Access denied")]
    AccessDenied,
    #[error("Database error: {0}")]
    Database(#[from] anyhow::Error),
    #[error("SeaORM error: {0}")]
    SeaOrm(#[from] sea_orm::DbErr),
}

pub struct PreviewUrlResolver {
    db: DbApi,
}

impl PreviewUrlResolver {
    pub fn new(db: DbApi) -> Self {
        Self { db }
    }

    /// Parse subdomain format: port-service-hash.app.lap.dev
    pub fn parse_preview_url(subdomain: &str) -> Result<PreviewUrlInfo, PreviewUrlError> {
        // Expected format: "8080-webapp-abc123"
        let parts: Vec<&str> = subdomain.split('-').collect();
        if parts.len() < 3 {
            return Err(PreviewUrlError::InvalidFormat);
        }

        let port: u16 = parts[0].parse().map_err(|_| PreviewUrlError::InvalidPort)?;
        let service_name = parts[1..parts.len() - 1].join("-"); // Handle multi-part service names
        let env_hash = parts[parts.len() - 1];

        Ok(PreviewUrlInfo {
            service_name,
            port,
            environment_hash: env_hash.to_string(),
        })
    }

    /// Resolve preview URL to target using existing database entities
    pub async fn resolve_preview_url(
        &self,
        info: PreviewUrlInfo,
    ) -> Result<PreviewUrlTarget, PreviewUrlError> {
        // 1. Find environment by hash (using name field as hash for now)
        let environment = self
            .db
            .find_environment_by_hash(&info.environment_hash)
            .await?
            .ok_or(PreviewUrlError::EnvironmentNotFound)?;

        // 2. Find matching service in kube_environment_service table
        let service = self
            .db
            .find_environment_service(environment.id, &info.service_name)
            .await?
            .ok_or(PreviewUrlError::ServiceNotFound)?;

        // 3. Find preview URL configuration in kube_environment_preview_url table
        let preview_url = self
            .db
            .find_preview_url_by_service_port(environment.id, service.id, info.port as i32)
            .await?
            .ok_or(PreviewUrlError::PreviewUrlNotConfigured)?;

        // 4. Validate access level and permissions (basic validation)
        let access_level = self.parse_access_level(&preview_url.access_level)?;

        Ok(PreviewUrlTarget {
            cluster_id: environment.cluster_id,
            namespace: environment.namespace,
            service_name: service.name,
            service_port: info.port,
            access_level,
            protocol: preview_url.protocol,
            environment_id: environment.id,
            organization_id: environment.organization_id,
            preview_url_id: preview_url.id,
        })
    }

    fn parse_access_level(
        &self,
        access_level: &str,
    ) -> Result<PreviewUrlAccessLevel, PreviewUrlError> {
        access_level
            .parse::<PreviewUrlAccessLevel>()
            .map_err(|_| PreviewUrlError::AccessDenied)
    }

    /// Update last accessed timestamp for the preview URL
    pub async fn update_preview_url_access(
        &self,
        preview_url_id: Uuid,
    ) -> Result<(), PreviewUrlError> {
        self.db
            .update_preview_url_access(preview_url_id)
            .await
            .map_err(PreviewUrlError::SeaOrm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_preview_url_simple() {
        let result = PreviewUrlResolver::parse_preview_url("8080-webapp-abc123");
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.service_name, "webapp");
        assert_eq!(info.port, 8080);
        assert_eq!(info.environment_hash, "abc123");
    }

    #[test]
    fn test_parse_preview_url_multi_part_service() {
        let result = PreviewUrlResolver::parse_preview_url("8080-my-web-app-abc123");
        assert!(result.is_ok());
        let info = result.unwrap();
        assert_eq!(info.service_name, "my-web-app");
        assert_eq!(info.port, 8080);
        assert_eq!(info.environment_hash, "abc123");
    }

    #[test]
    fn test_parse_preview_url_invalid_format() {
        let result = PreviewUrlResolver::parse_preview_url("8080-webapp");
        assert!(matches!(result, Err(PreviewUrlError::InvalidFormat)));
    }

    #[test]
    fn test_parse_preview_url_invalid_port() {
        let result = PreviewUrlResolver::parse_preview_url("abc-webapp-xyz123");
        assert!(matches!(result, Err(PreviewUrlError::InvalidPort)));
    }
}
