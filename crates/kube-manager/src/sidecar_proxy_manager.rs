use anyhow::Result;
use futures::StreamExt;
use k8s_openapi::api::core::v1::Service;
use kube::{api::ListParams, Api, Client as KubeClient};
use lapdev_common::kube::{DEFAULT_SIDECAR_PROXY_MANAGER_PORT, SIDECAR_PROXY_MANAGER_PORT_ENV_VAR};
use lapdev_kube_rpc::{ProxyRouteConfig, SidecarProxyManagerRpc, SidecarProxyRpcClient};
use lapdev_rpc::spawn_twoway;
use std::{collections::HashMap, sync::Arc};
use tarpc::server::{BaseChannel, Channel};
use tokio::sync::RwLock;
use tracing::{error, info};
use uuid::Uuid;

use crate::sidecar_proxy_manager_rpc::SidecarProxyManagerRpcServer;

#[derive(Clone)]
pub struct SidecarProxyManager {
    kube_client: KubeClient,
    pub(crate) sidecar_proxies: Arc<RwLock<HashMap<Uuid, SidecarProxyRegistration>>>,
}

#[derive(Clone)]
pub(crate) struct SidecarProxyRegistration {
    pub workload_id: Uuid,
    pub namespace: String,
    pub rpc_client: SidecarProxyRpcClient,
}

impl SidecarProxyManager {
    pub(crate) async fn new(kube_client: KubeClient) -> Result<Self> {
        // Parse the URL to extract the port
        let port = std::env::var(SIDECAR_PROXY_MANAGER_PORT_ENV_VAR)
            .ok()
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(DEFAULT_SIDECAR_PROXY_MANAGER_PORT);
        let bind_addr = format!("0.0.0.0:{port}");
        info!("Starting TCP server for sidecar proxies on: {}", bind_addr);

        let mut listener = tarpc::serde_transport::tcp::listen(
            ("0.0.0.0", port),
            tarpc::tokio_serde::formats::Bincode::default,
        )
        .await?;
        info!("TCP server listening on: {}", bind_addr);

        let m = Self {
            kube_client,
            sidecar_proxies: Arc::new(RwLock::new(HashMap::new())),
        };

        {
            let m = m.clone();
            tokio::spawn(async move {
                while let Some(conn) = listener.next().await {
                    if let Ok(conn) = conn {
                        let m = m.clone();
                        tokio::spawn(async move {
                            let (server_chan, client_chan, _) = spawn_twoway(conn);
                            let rpc_client = SidecarProxyRpcClient::new(
                                tarpc::client::Config::default(),
                                client_chan,
                            )
                            .spawn();
                            let rpc_server =
                                SidecarProxyManagerRpcServer::new(m.clone(), rpc_client);
                            BaseChannel::with_defaults(server_chan)
                                .execute(rpc_server.serve())
                                .for_each(|resp| async move {
                                    tokio::spawn(resp);
                                })
                                .await;
                        });
                    }
                }
                error!("TCP connection stopped");
            });
        }

        Ok(m)
    }

    pub async fn set_service_routes_if_registered(&self, environment_id: Uuid) -> Result<()> {
        let registration = {
            let map = self.sidecar_proxies.read().await;
            map.get(&environment_id).cloned()
        };

        let Some(registration) = registration else {
            return Ok(());
        };

        let scoped_services = {
            let selector = format!("lapdev.base-workload-id={}", registration.workload_id);
            let services_api_all: Api<Service> = Api::all(self.kube_client.clone());
            let listed = services_api_all
                .list(&ListParams::default().labels(&selector))
                .await?
                .items;

            if listed.is_empty() {
                // Backward compatibility: fall back to namespace-scoped lookup
                let services_api_ns: Api<Service> =
                    Api::namespaced(self.kube_client.clone(), &registration.namespace);
                services_api_ns.list(&ListParams::default()).await?.items
            } else {
                listed
            }
        };

        let mut routes = Vec::new();
        for svc in scoped_services {
            let svc_name = svc.metadata.name.unwrap_or_default();
            if svc_name.is_empty() {
                continue;
            }

            let branch_env_id = svc
                .metadata
                .labels
                .as_ref()
                .and_then(|labels| labels.get("lapdev.io/branch-environment-id"))
                .and_then(|value| Uuid::parse_str(value).ok());

            if let Some(spec) = svc.spec {
                if let Some(ports) = spec.ports {
                    for port in ports {
                        let Ok(port_number) = u16::try_from(port.port) else {
                            continue;
                        };
                        routes.push(ProxyRouteConfig {
                            path: format!("/{}/*", svc_name),
                            service_name: svc_name.clone(),
                            port: port_number,
                            branch_environment_id: branch_env_id,
                        });
                    }
                }
            }
        }

        if let Err(e) = registration
            .rpc_client
            .set_service_routes(tarpc::context::current(), routes)
            .await
        {
            return Err(anyhow::anyhow!("Failed to push service routes: {}", e));
        }

        Ok(())
    }
}
