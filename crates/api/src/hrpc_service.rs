use axum_extra::headers;
use chrono::{DateTime, Utc};
use lapdev_api_hrpc::HrpcService;
use lapdev_common::{
    devbox::{
        DevboxEnvironmentSelection, DevboxPortMapping, DevboxPortMappingOverride,
        DevboxSessionSummary, DevboxSessionWhoAmI, DevboxStartWorkloadInterceptResponse,
        DevboxWorkloadInterceptListResponse, DevboxWorkloadInterceptSummary,
    },
    hrpc::HrpcError,
    kube::{
        CreateKubeClusterResponse, KubeAppCatalog, KubeAppCatalogWorkload,
        KubeAppCatalogWorkloadCreate, KubeCluster, KubeEnvironment,
        KubeEnvironmentDashboardSummary, KubeEnvironmentWorkload, KubeEnvironmentWorkloadDetail,
        KubeNamespace, KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind, KubeWorkloadList,
        PagePaginationParams, PaginatedResult, PaginationParams,
    },
    UserRole,
};
use lapdev_rpc::error::ApiError;
use pasetors::claims::Claims;
use sea_orm::DbErr;
use tarpc::context;
use uuid::Uuid;

use crate::state::CoreState;

impl HrpcService for CoreState {
    async fn devbox_session_get_session(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<Option<DevboxSessionSummary>, HrpcError> {
        let user = self.hrpc_authenticate_user(headers).await?;
        let session = self
            .db
            .get_active_devbox_session(user.id)
            .await
            .map_err(hrpc_from_anyhow)?;

        let session = session.map(|session| DevboxSessionSummary {
            id: session.id,
            device_name: session.device_name,
            token_prefix: session.token_prefix,
            active_environment_id: session.active_environment_id,
            created_at: session.created_at.with_timezone(&Utc),
            expires_at: session.expires_at.with_timezone(&Utc),
            last_used_at: session.last_used_at.with_timezone(&Utc),
            revoked_at: session.revoked_at.map(|ts| ts.with_timezone(&Utc)),
        });

        Ok(session)
    }

    async fn devbox_session_revoke_session(
        &self,
        headers: &axum::http::HeaderMap,
        session_id: Uuid,
    ) -> Result<(), HrpcError> {
        let user = self.hrpc_authenticate_user(headers).await?;
        let session = self
            .db
            .get_devbox_session_by_id(session_id)
            .await
            .map_err(hrpc_from_anyhow)?
            .ok_or_else(|| hrpc_error("Session not found"))?;

        if session.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }

        if session.revoked_at.is_some() {
            return Ok(());
        }

        self.db
            .revoke_devbox_session_by_id(session_id)
            .await
            .map_err(hrpc_from_anyhow)?;

        let session_notification = {
            let sessions = self.active_devbox_sessions.read().await;
            sessions.get(&user.id).and_then(|entries| {
                entries
                    .iter()
                    .find(|(_, handle)| handle.session_id == session_id)
                    .map(|(connection_id, handle)| (*connection_id, handle.rpc_client.clone()))
            })
        };

        if let Some((connection_id, client)) = session_notification {
            {
                let mut sessions = self.active_devbox_sessions.write().await;
                if let Some(entries) = sessions.get_mut(&user.id) {
                    entries.remove(&connection_id);
                    if entries.is_empty() {
                        sessions.remove(&user.id);
                    }
                }
            }
            tokio::spawn(async move {
                let _ = client
                    .session_displaced(
                        context::current(),
                        "Session revoked by dashboard".to_string(),
                    )
                    .await;
            });
        }

        Ok(())
    }

    async fn devbox_session_get_active_environment(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<Option<DevboxEnvironmentSelection>, HrpcError> {
        let ctx = self.hrpc_resolve_active_devbox_session(headers).await?;

        let Some(environment_id) = ctx.session.active_environment_id else {
            return Ok(None);
        };

        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?;

        let Some(environment) = environment else {
            return Ok(None);
        };

        let cluster = self
            .db
            .get_kube_cluster(environment.cluster_id)
            .await
            .map_err(hrpc_from_anyhow)?
            .ok_or_else(|| hrpc_error("Cluster not found"))?;

        Ok(Some(DevboxEnvironmentSelection {
            environment_id,
            cluster_name: cluster.name,
            namespace: environment.namespace,
        }))
    }

    async fn devbox_session_set_active_environment(
        &self,
        headers: &axum::http::HeaderMap,
        environment_id: Uuid,
    ) -> Result<(), HrpcError> {
        let ctx = self.hrpc_resolve_active_devbox_session(headers).await?;
        let previous_environment = ctx.session.active_environment_id;
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.is_shared {
            return Err(hrpc_error(
                "Shared environments cannot be used as active devbox environments",
            ));
        }

        self.ensure_environment_access(&ctx.user, &environment)
            .await?;

        self.db
            .update_devbox_session_active_environment(ctx.session.user_id, Some(environment_id))
            .await
            .map_err(hrpc_from_anyhow)?;

        // Notify CLI about the environment change
        let cluster = self
            .db
            .get_kube_cluster(environment.cluster_id)
            .await
            .map_err(hrpc_from_anyhow)?
            .ok_or_else(|| hrpc_error("Cluster not found"))?;

        let env_info = lapdev_devbox_rpc::DevboxEnvironmentInfo {
            environment_id: environment.id,
            environment_name: environment.name.clone(),
            cluster_name: cluster.name.clone(),
            namespace: environment.namespace.clone(),
        };

        tracing::info!(
            "set_active_environment (HRPC) called for session {} with environment {} ({}/{})",
            ctx.session.id,
            environment_id,
            cluster.name,
            environment.namespace
        );

        // Get the RPC client for the active session
        let rpc_client = {
            let sessions = self.active_devbox_sessions.read().await;
            sessions.get(&ctx.user.id).and_then(|entries| {
                entries
                    .values()
                    .next_back()
                    .map(|handle| handle.rpc_client.clone())
            })
        };

        if let Some(client) = rpc_client {
            tracing::info!(
                "Spawning task to notify CLI about environment change to {} ({})",
                env_info.cluster_name,
                env_info.namespace
            );
            tokio::spawn(async move {
                tracing::info!("Calling environment_changed RPC on client...");
                match client
                    .environment_changed(context::current(), Some(env_info.clone()))
                    .await
                {
                    Ok(_) => {
                        tracing::info!(
                            "Successfully notified client of environment change to {}",
                            env_info.namespace
                        );
                    }
                    Err(e) => {
                        tracing::warn!("Failed to notify client of environment change: {}", e);
                    }
                }
            });
        } else {
            tracing::warn!(
                "No active WebSocket connection for user {}, environment change saved but CLI not notified",
                ctx.user.id
            );
        }

        let state = self.clone();
        let user_id = ctx.user.id;
        tokio::spawn(async move {
            state.push_devbox_routes(user_id, environment_id).await;
        });

        if let Some(prev_env) = previous_environment.filter(|prev| *prev != environment_id) {
            let state = self.clone();
            tokio::spawn(async move {
                state.clear_devbox_routes_for_environment(prev_env).await;
            });
        }

        Ok(())
    }

    async fn devbox_session_whoami(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<DevboxSessionWhoAmI, HrpcError> {
        let ctx = self.hrpc_resolve_active_devbox_session(headers).await?;

        Ok(DevboxSessionWhoAmI {
            user_id: ctx.user.id,
            email: ctx.user.email.clone(),
            device_name: ctx.session.device_name,
            authenticated_at: ctx.authenticated_at,
            expires_at: ctx.expires_at,
        })
    }

    async fn devbox_intercept_list(
        &self,
        headers: &axum::http::HeaderMap,
        environment_id: Uuid,
    ) -> Result<DevboxWorkloadInterceptListResponse, HrpcError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        let user = self
            .authorize(headers, environment.organization_id, None)
            .await
            .map_err(hrpc_from_api_error)?;

        self.ensure_environment_access(&user, &environment).await?;

        let intercepts = self
            .db
            .list_workload_intercepts_for_environment(environment_id)
            .await
            .map_err(hrpc_from_anyhow)?;

        let mut results = Vec::with_capacity(intercepts.len());
        for intercept in intercepts {
            let workload = self
                .db
                .get_environment_workload(intercept.workload_id)
                .await
                .map_err(hrpc_from_anyhow)?
                .ok_or_else(|| hrpc_error("Workload not found"))?;

            let port_mappings: Vec<DevboxPortMapping> =
                serde_json::from_value(intercept.port_mappings.clone())
                    .map_err(hrpc_from_anyhow)?;

            results.push(DevboxWorkloadInterceptSummary {
                intercept_id: intercept.id,
                session_id: Uuid::nil(), // Intercepts are no longer tied to sessions
                workload_id: workload.id,
                workload_name: workload.name,
                namespace: workload.namespace,
                port_mappings,
                created_at: intercept.created_at.with_timezone(&Utc),
                restored_at: intercept.stopped_at.map(|dt| dt.with_timezone(&Utc)),
            });
        }

        Ok(DevboxWorkloadInterceptListResponse {
            intercepts: results,
        })
    }

    async fn devbox_intercept_start(
        &self,
        headers: &axum::http::HeaderMap,
        workload_id: Uuid,
        port_mappings: Vec<DevboxPortMappingOverride>,
    ) -> Result<DevboxStartWorkloadInterceptResponse, HrpcError> {
        let user = self.hrpc_authenticate_user(headers).await?;

        let workload = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(hrpc_from_anyhow)?
            .ok_or_else(|| hrpc_error("Workload not found"))?;

        let environment = self
            .db
            .get_kube_environment(workload.environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.is_shared {
            return Err(hrpc_error(
                "Shared environments cannot start Devbox intercepts",
            ));
        }

        self.ensure_environment_access(&user, &environment).await?;

        let mappings = port_mappings
            .into_iter()
            .map(|override_mapping| DevboxPortMapping {
                workload_port: override_mapping.workload_port,
                local_port: override_mapping
                    .local_port
                    .unwrap_or(override_mapping.workload_port),
                protocol: "TCP".to_string(),
            })
            .collect::<Vec<_>>();

        let intercept = self
            .db
            .create_workload_intercept(
                user.id,
                environment.id,
                workload_id,
                serde_json::to_value(&mappings).map_err(hrpc_from_anyhow)?,
            )
            .await
            .map_err(hrpc_from_anyhow)?;

        if self
            .active_devbox_sessions
            .read()
            .await
            .get(&user.id)
            .is_some()
        {
            let state = self.clone();
            let environment = environment.clone();
            let intercept_clone = intercept.clone();
            tokio::spawn(async move {
                state
                    .push_devbox_route_for_intercept(environment, intercept_clone)
                    .await;
            });
        }

        Ok(DevboxStartWorkloadInterceptResponse {
            intercept_id: intercept.id,
        })
    }

    async fn devbox_intercept_stop(
        &self,
        headers: &axum::http::HeaderMap,
        intercept_id: Uuid,
    ) -> Result<(), HrpcError> {
        let user = self.hrpc_authenticate_user(headers).await?;

        let intercept = self
            .db
            .get_workload_intercept(intercept_id)
            .await
            .map_err(hrpc_from_anyhow)?
            .ok_or_else(|| hrpc_error("Intercept not found"))?;

        if intercept.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }

        self.db
            .stop_workload_intercept(intercept_id)
            .await
            .map_err(hrpc_from_anyhow)?;
        if self
            .active_devbox_sessions
            .read()
            .await
            .get(&user.id)
            .is_some()
        {
            match self.db.get_kube_environment(intercept.environment_id).await {
                Ok(Some(environment)) => {
                    let state = self.clone();
                    let intercept_clone = intercept.clone();
                    tokio::spawn(async move {
                        state
                            .clear_devbox_route_for_intercept(environment, intercept_clone)
                            .await;
                    });
                }
                Ok(None) => {
                    tracing::warn!(
                        environment_id = %intercept.environment_id,
                        intercept_id = %intercept.id,
                        "Environment not found while clearing devbox route after intercept stop"
                    );
                }
                Err(err) => {
                    tracing::warn!(
                        environment_id = %intercept.environment_id,
                        intercept_id = %intercept.id,
                        error = %err,
                        "Failed to load environment while clearing devbox route after intercept stop"
                    );
                }
            }
        }

        Ok(())
    }

    async fn all_kube_clusters(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
    ) -> Result<Vec<KubeCluster>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_all_kube_clusters(org_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
    ) -> Result<CreateKubeClusterResponse, HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .create_kube_cluster(org_id, user.id, name)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_kube_cluster(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .delete_kube_cluster(org_id, cluster_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn set_cluster_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy_personal: bool,
        can_deploy_shared: bool,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .set_cluster_deployable(org_id, cluster_id, can_deploy_personal, can_deploy_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn set_cluster_personal_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .set_cluster_personal_deployable(org_id, cluster_id, can_deploy)
            .await
            .map_err(HrpcError::from)
    }

    async fn set_cluster_shared_deployable(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        can_deploy: bool,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .set_cluster_shared_deployable(org_id, cluster_id, can_deploy)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_workloads(
                org_id,
                cluster_id,
                namespace,
                workload_kind_filter,
                include_system_workloads,
                pagination,
            )
            .await
            .map_err(HrpcError::from)
    }

    async fn get_workload_details(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_workload_details(org_id, cluster_id, name, namespace)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_cluster_namespaces(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<Vec<KubeNamespaceInfo>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_cluster_namespaces(org_id, cluster_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_cluster_info(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
    ) -> Result<KubeCluster, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_cluster_info(org_id, cluster_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        cluster_id: Uuid,
        name: String,
        description: Option<String>,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<Uuid, HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .create_app_catalog(org_id, user.id, cluster_id, name, description, workloads)
            .await
            .map_err(HrpcError::from)
    }

    async fn all_app_catalogs(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeAppCatalog>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_app_catalogs(org_id, search, pagination)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<KubeAppCatalog, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_app_catalog(org_id, catalog_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_app_catalog_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<Vec<KubeAppCatalogWorkload>, HrpcError> {
        let _ = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_app_catalog_workloads(org_id, catalog_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn all_kube_environments(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        search: Option<String>,
        is_shared: bool,
        is_branch: bool,
        pagination: Option<PagePaginationParams>,
    ) -> Result<PaginatedResult<KubeEnvironment>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_kube_environments(org_id, user.id, search, is_shared, is_branch, pagination)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_dashboard_summary(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        recent_limit: Option<usize>,
    ) -> Result<KubeEnvironmentDashboardSummary, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_environment_dashboard_summary(org_id, user.id, recent_limit)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<KubeEnvironment, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_kube_environment(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.organization_id != org_id {
            return Err(hrpc_error(
                "Environment does not belong to the organization",
            ));
        }

        let required_role = environment.is_shared.then_some(UserRole::Admin);
        let user = self.authorize(headers, org_id, required_role).await?;

        if !environment.is_shared && environment.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }

        self.kube_controller
            .delete_kube_environment(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn pause_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.organization_id != org_id {
            return Err(hrpc_error(
                "Environment does not belong to the organization",
            ));
        }

        if environment.is_shared {
            return Err(hrpc_error(
                "Pause is not yet supported for shared environments",
            ));
        }

        let user = self.authorize(headers, org_id, None).await?;

        if environment.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }
        self.kube_controller
            .pause_kube_environment(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn resume_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.organization_id != org_id {
            return Err(hrpc_error(
                "Environment does not belong to the organization",
            ));
        }

        if environment.is_shared {
            return Err(hrpc_error(
                "Resume is not yet supported for shared environments",
            ));
        }

        let user = self.authorize(headers, org_id, None).await?;

        if environment.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }
        self.kube_controller
            .resume_kube_environment(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn sync_environment_from_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<(), HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .sync_environment_from_catalog(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .delete_app_catalog(org_id, catalog_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_app_catalog_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .delete_app_catalog_workload(org_id, workload_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn update_app_catalog_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
    ) -> Result<(), HrpcError> {
        let _ = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .update_app_catalog_workload(org_id, workload_id, containers)
            .await
            .map_err(HrpcError::from)
    }

    async fn add_workloads_to_app_catalog(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        catalog_id: Uuid,
        workloads: Vec<KubeAppCatalogWorkloadCreate>,
    ) -> Result<(), HrpcError> {
        let user = self
            .authorize(headers, org_id, Some(UserRole::Admin))
            .await?;

        self.kube_controller
            .add_workloads_to_app_catalog(org_id, user.id, catalog_id, workloads)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_kube_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        app_catalog_id: Uuid,
        cluster_id: Uuid,
        name: String,
        is_shared: bool,
    ) -> Result<KubeEnvironment, HrpcError> {
        let required_role = is_shared.then_some(UserRole::Admin);
        let user = self.authorize(headers, org_id, required_role).await?;

        self.kube_controller
            .create_kube_environment(org_id, user.id, app_catalog_id, cluster_id, name, is_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_branch_environment(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        base_environment_id: Uuid,
        name: String,
    ) -> Result<KubeEnvironment, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .create_branch_environment(org_id, user.id, base_environment_id, name)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_workloads(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<KubeEnvironmentWorkload>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_workloads(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_services(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentService>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_services(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<Option<KubeEnvironmentWorkload>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_workload(org_id, user.id, workload_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_workload_detail(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
    ) -> Result<KubeEnvironmentWorkloadDetail, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_workload_detail(org_id, user.id, environment_id, workload_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn update_environment_workload(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
    ) -> Result<Uuid, HrpcError> {
        let workload = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(|e| HrpcError::from(ApiError::from(e)))?
            .ok_or_else(|| {
                HrpcError::from(ApiError::InvalidRequest("Workload not found".to_string()))
            })?;

        let target_environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(|e| HrpcError::from(ApiError::from(e)))?
            .ok_or_else(|| {
                HrpcError::from(ApiError::InvalidRequest(
                    "Environment not found".to_string(),
                ))
            })?;

        if target_environment.organization_id != org_id {
            return Err(HrpcError::from(ApiError::Unauthorized));
        }

        // Ensure the workload belongs to this environment or its base
        if workload.environment_id != target_environment.id {
            let base_env = target_environment.base_environment_id.ok_or_else(|| {
                // Branch workloads reference their base shared environment; if there's no base we can't continue
                HrpcError::from(ApiError::InvalidRequest(
                    "Workload does not belong to the target environment".to_string(),
                ))
            })?;
            if workload.environment_id != base_env {
                return Err(HrpcError::from(ApiError::InvalidRequest(
                    "Workload does not belong to the target environment".to_string(),
                )));
            }
        }

        // Determine required role based on the target environment. Branch workloads inherit permissions from their branch.
        let required_role = target_environment.is_shared.then_some(UserRole::Admin);

        let user = self.authorize(headers, org_id, required_role).await?;

        if !target_environment.is_shared && target_environment.user_id != user.id {
            return Err(HrpcError::from(ApiError::Unauthorized));
        }

        self.kube_controller
            .update_environment_workload(
                org_id,
                user.id,
                workload_id,
                containers,
                target_environment,
            )
            .await
            .map_err(HrpcError::from)
    }

    async fn create_kube_namespace(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        name: String,
        description: Option<String>,
        is_shared: bool,
    ) -> Result<KubeNamespace, HrpcError> {
        // Only admin can create shared namespaces, regular users can create personal ones
        let required_role = if is_shared {
            Some(UserRole::Admin)
        } else {
            None
        };
        let user = self.authorize(headers, org_id, required_role).await?;

        self.kube_controller
            .create_kube_namespace(org_id, user.id, name, description, is_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn all_kube_namespaces(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        is_shared: bool,
    ) -> Result<Vec<KubeNamespace>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;

        self.kube_controller
            .get_all_kube_namespaces(org_id, user.id, is_shared)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_kube_namespace(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        namespace_id: Uuid,
    ) -> Result<(), HrpcError> {
        // Get the namespace first to check if it's shared
        let namespace = self
            .kube_controller
            .db
            .get_kube_namespace(namespace_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Namespace not found".to_string()))?;

        if namespace.organization_id != org_id {
            return Err(ApiError::Unauthorized)?;
        }

        // For shared namespaces, only admin can delete
        // For personal namespaces, any user can delete (with ownership check below)
        let required_role = if namespace.is_shared {
            Some(UserRole::Admin)
        } else {
            None
        };
        let user = self.authorize(headers, org_id, required_role).await?;

        // For personal namespaces, only the creator can delete
        if !namespace.is_shared && namespace.user_id != user.id {
            return Err(ApiError::InvalidRequest(
                "You can only delete namespaces you created".to_string(),
            ))?;
        }

        self.kube_controller
            .delete_kube_namespace(org_id, namespace_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn create_environment_preview_url(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
        request: lapdev_common::kube::CreateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<lapdev_common::kube::KubeEnvironmentPreviewUrl, HrpcError> {
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.organization_id != org_id {
            return Err(hrpc_error(
                "Environment does not belong to the organization",
            ));
        }

        let required_role = environment.is_shared.then_some(UserRole::Admin);
        let user = self.authorize(headers, org_id, required_role).await?;

        if !environment.is_shared && environment.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }

        self.kube_controller
            .create_environment_preview_url(user.id, environment, request)
            .await
            .map_err(HrpcError::from)
    }

    async fn get_environment_preview_urls(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentPreviewUrl>, HrpcError> {
        let user = self.authorize(headers, org_id, None).await?;
        self.kube_controller
            .get_environment_preview_urls(org_id, user.id, environment_id)
            .await
            .map_err(HrpcError::from)
    }

    async fn update_environment_preview_url(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        preview_url_id: Uuid,
        request: lapdev_common::kube::UpdateKubeEnvironmentPreviewUrlRequest,
    ) -> Result<lapdev_common::kube::KubeEnvironmentPreviewUrl, HrpcError> {
        let preview_url = self
            .db
            .get_environment_preview_url(preview_url_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Preview URL not found"))?;

        let environment = self
            .db
            .get_kube_environment(preview_url.environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.organization_id != org_id {
            return Err(hrpc_error(
                "Environment does not belong to the organization",
            ));
        }

        let required_role = environment.is_shared.then_some(UserRole::Admin);
        let user = self.authorize(headers, org_id, required_role).await?;

        if !environment.is_shared && environment.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }

        self.kube_controller
            .update_environment_preview_url(preview_url_id, request)
            .await
            .map_err(HrpcError::from)
    }

    async fn delete_environment_preview_url(
        &self,
        headers: &axum::http::HeaderMap,
        org_id: Uuid,
        preview_url_id: Uuid,
    ) -> Result<(), HrpcError> {
        let preview_url = self
            .db
            .get_environment_preview_url(preview_url_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Preview URL not found"))?;

        let environment = self
            .db
            .get_kube_environment(preview_url.environment_id)
            .await
            .map_err(hrpc_from_db_err)?
            .ok_or_else(|| hrpc_error("Environment not found"))?;

        if environment.organization_id != org_id {
            return Err(hrpc_error(
                "Environment does not belong to the organization",
            ));
        }

        let required_role = environment.is_shared.then_some(UserRole::Admin);
        let user = self.authorize(headers, org_id, required_role).await?;

        if !environment.is_shared && environment.user_id != user.id {
            return Err(hrpc_error("Unauthorized"));
        }

        self.kube_controller
            .delete_environment_preview_url(preview_url_id)
            .await
            .map_err(HrpcError::from)
    }
}

struct ActiveDevboxSessionContext {
    user: lapdev_db_entities::user::Model,
    session: lapdev_db_entities::kube_devbox_session::Model,
    authenticated_at: Option<DateTime<Utc>>,
    expires_at: Option<DateTime<Utc>>,
}

impl CoreState {
    async fn hrpc_authenticate_user(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<lapdev_db_entities::user::Model, HrpcError> {
        self.authenticate_raw(headers)
            .await
            .map_err(hrpc_from_api_error)
    }

    async fn hrpc_resolve_active_devbox_session(
        &self,
        headers: &axum::http::HeaderMap,
    ) -> Result<ActiveDevboxSessionContext, HrpcError> {
        if let Some(value) = headers.get(axum::http::header::AUTHORIZATION) {
            let value = value
                .to_str()
                .map_err(|_| hrpc_error("Invalid Authorization header"))?;

            if let Some(token) = value.strip_prefix("Bearer ") {
                let bearer = headers::Authorization::bearer(token)
                    .map_err(|_| hrpc_error("Invalid bearer token"))?;
                let ctx = self
                    .authenticate_bearer(&bearer)
                    .await
                    .map_err(hrpc_from_api_error)?;

                let session = self
                    .db
                    .get_devbox_session_by_token_hash(token)
                    .await
                    .map_err(hrpc_from_anyhow)?
                    .ok_or_else(|| hrpc_error("Active session not found"))?;

                self.db
                    .update_devbox_session_last_used(session.user_id)
                    .await
                    .map_err(hrpc_from_anyhow)?;

                return Ok(ActiveDevboxSessionContext {
                    user: ctx.user,
                    session,
                    authenticated_at: parse_claim_datetime(&ctx.token_claims, "iat"),
                    expires_at: parse_claim_datetime(&ctx.token_claims, "exp"),
                });
            }
        }

        let user = self.hrpc_authenticate_user(headers).await?;
        let session = self
            .db
            .get_active_devbox_session(user.id)
            .await
            .map_err(hrpc_from_anyhow)?
            .ok_or_else(|| hrpc_error("No active devbox session"))?;

        self.db
            .update_devbox_session_last_used(session.user_id)
            .await
            .map_err(hrpc_from_anyhow)?;

        Ok(ActiveDevboxSessionContext {
            authenticated_at: Some(session.created_at.with_timezone(&Utc)),
            expires_at: Some(session.expires_at.with_timezone(&Utc)),
            user,
            session,
        })
    }

    async fn ensure_environment_access(
        &self,
        user: &lapdev_db_entities::user::Model,
        environment: &lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), HrpcError> {
        if environment.user_id == user.id {
            return Ok(());
        }

        if environment.is_shared {
            self.db
                .get_organization_member(user.id, environment.organization_id)
                .await
                .map(|_| ())
                .map_err(|_| hrpc_error("Unauthorized"))
        } else {
            Err(hrpc_error("Unauthorized"))
        }
    }
}

fn hrpc_error<E: ToString>(err: E) -> HrpcError {
    HrpcError {
        error: err.to_string(),
    }
}

fn hrpc_from_anyhow<E: ToString>(err: E) -> HrpcError {
    hrpc_error(err)
}

fn hrpc_from_api_error(err: ApiError) -> HrpcError {
    hrpc_error(err)
}

fn hrpc_from_db_err(err: DbErr) -> HrpcError {
    hrpc_error(err)
}

fn parse_claim_datetime(claims: &Claims, key: &str) -> Option<DateTime<Utc>> {
    claims
        .get_claim(key)
        .and_then(|value| serde_json::from_value::<String>(value.clone()).ok())
        .and_then(|value| chrono::DateTime::parse_from_rfc3339(&value).ok())
        .map(|dt| dt.with_timezone(&Utc))
}
