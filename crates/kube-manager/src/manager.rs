use std::{collections::HashMap, fs, net::SocketAddr, sync::Arc, time::Instant};

use anyhow::{anyhow, Result};
use chrono::Utc;
use futures::StreamExt;
use k8s_openapi::{
    api::{
        apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
        batch::v1::{CronJob, Job},
        core::v1::{ConfigMap, Namespace, Pod, Secret, Service},
    },
    NamespaceResourceScope,
};
use kube::api::{DeleteParams, ListParams};
use lapdev_common::{
    devbox::{DirectTunnelConfig, DirectTunnelCredential},
    kube::{
        KubeClusterInfo, KubeClusterStatus, KubeNamespaceInfo, KubeWorkload, KubeWorkloadKind,
        KubeWorkloadList, KubeWorkloadStatus, PaginationCursor, PaginationParams,
        DEFAULT_KUBE_CLUSTER_TUNNEL_URL, DEFAULT_KUBE_CLUSTER_URL, KUBE_CLUSTER_TOKEN_ENV_VAR,
        KUBE_CLUSTER_TOKEN_HEADER, KUBE_CLUSTER_TUNNEL_URL_ENV_VAR, KUBE_CLUSTER_URL_ENV_VAR,
    },
};
use lapdev_kube_rpc::{
    DevboxRouteConfig, KubeClusterRpcClient, KubeManagerRpc, KubeWorkloadYamlOnly,
    KubeWorkloadsWithResources, NamespacedResourceRequest, NamespacedResourceResponse,
    ProxyBranchRouteConfig,
};
use lapdev_rpc::spawn_twoway;
use lapdev_tunnel::{direct::DirectEndpoint, websocket_serde_transport};
use tarpc::server::{BaseChannel, Channel};
use tokio::{
    task::JoinHandle,
    time::{sleep, Duration, MissedTickBehavior},
};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use uuid::Uuid;

use crate::manager_rpc::KubeManagerRpcServer;
use crate::{
    devbox_proxy_manager::DevboxProxyManager, sidecar_proxy_manager::SidecarProxyManager,
    tunnel::TunnelManager, watch_manager::WatchManager,
};

const CLUSTER_HEARTBEAT_INTERVAL_SECS: u64 = 30;

#[derive(Clone)]
pub struct KubeManager {
    pub(crate) kube_client: Arc<kube::Client>,
    // rpc_client: KubeClusterRpcClient,
    proxy_manager: Arc<SidecarProxyManager>,
    pub(crate) devbox_proxy_manager: Arc<DevboxProxyManager>,
    tunnel_manager: TunnelManager,
    pub(crate) watch_manager: Arc<WatchManager>,
    manager_namespace: Option<String>,
    direct_endpoint: Arc<DirectEndpoint>,
}

impl KubeManager {
    pub async fn connect_cluster() -> Result<()> {
        let kube_client = Arc::new(Self::new_kube_client().await.map_err(|e| {
            tracing::error!("Failed to create Kubernetes client: {}", e);
            e
        })?);

        let proxy_manager = Arc::new(SidecarProxyManager::new().await?);
        let devbox_proxy_manager = Arc::new(DevboxProxyManager::new().await?);
        let watch_manager = Arc::new(WatchManager::new(kube_client.clone()));
        let direct_endpoint = Arc::new(
            DirectEndpoint::bind()
                .await
                .map_err(|e| anyhow::anyhow!("failed to initialize direct endpoint: {e}"))?,
        );
        {
            let direct_endpoint = Arc::clone(&direct_endpoint);
            tokio::spawn(async move {
                direct_endpoint.start_tunnel_server().await;
            });
        }
        let token = std::env::var(KUBE_CLUSTER_TOKEN_ENV_VAR)
            .map_err(|_| anyhow::anyhow!("can't find env var {}", KUBE_CLUSTER_TOKEN_ENV_VAR))?;
        let url = std::env::var(KUBE_CLUSTER_URL_ENV_VAR)
            .unwrap_or_else(|_| DEFAULT_KUBE_CLUSTER_URL.to_string());
        let tunnel_url = std::env::var(KUBE_CLUSTER_TUNNEL_URL_ENV_VAR)
            .unwrap_or_else(|_| DEFAULT_KUBE_CLUSTER_TUNNEL_URL.to_string());

        tracing::info!("Connecting to Lapdev cluster at: {}", url);

        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert(KUBE_CLUSTER_TOKEN_HEADER, token.parse()?);

        let mut tunnel_request = tunnel_url.into_client_request()?;
        tunnel_request
            .headers_mut()
            .insert(KUBE_CLUSTER_TOKEN_HEADER, token.parse()?);

        let manager_namespace = Self::detect_manager_namespace();

        let manager = KubeManager {
            kube_client,
            proxy_manager,
            devbox_proxy_manager,
            tunnel_manager: TunnelManager::new(tunnel_request, token.clone()),
            watch_manager,
            manager_namespace,
            direct_endpoint,
        };

        // Start the tunnel manager connection cycle in the background
        let tunnel_manager = manager.tunnel_manager.clone();
        let tunnel_task = tokio::spawn(async move {
            if let Err(e) = tunnel_manager.start_tunnel_cycle().await {
                tracing::error!("Tunnel connection cycle failed: {}", e);
            }
        });

        // Start the main RPC connection cycle
        let main_task = tokio::spawn(async move {
            loop {
                match manager.handle_connection_cycle(request.clone()).await {
                    Ok(_) => {
                        tracing::warn!("Connection cycle completed, will retry in 5 second...");
                    }
                    Err(e) => {
                        tracing::warn!("Connection cycle failed: {}, retrying in 5 seconds...", e);
                    }
                }
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
            }
        });

        // Wait for either task to complete (they should run forever)
        tokio::select! {
            result = tunnel_task => {
                tracing::error!("Tunnel task completed unexpectedly: {:?}", result);
                return Err(anyhow!("Tunnel task completed unexpectedly"));
            }
            result = main_task => {
                tracing::error!("Main task completed unexpectedly: {:?}", result);
                return Err(anyhow!("Main task completed unexpectedly"));
            }
        }
    }

    async fn handle_connection_cycle(
        &self,
        request: tokio_tungstenite::tungstenite::http::Request<()>,
    ) -> Result<()> {
        tracing::info!("Attempting to connect to cluster...");

        let (stream, _) = tokio_tungstenite::connect_async(request.clone()).await?;

        tracing::info!("WebSocket connection established");

        let transport =
            websocket_serde_transport(stream, tarpc::tokio_serde::formats::Bincode::default());
        let (server_chan, client_chan, _) = spawn_twoway(transport);

        let rpc_client =
            KubeClusterRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

        self.watch_manager.set_rpc_client(rpc_client.clone()).await;
        self.proxy_manager
            .set_cluster_rpc_client(rpc_client.clone())
            .await;

        let rpc_server = KubeManagerRpcServer::new(self.clone(), rpc_client.clone());

        // Spawn the WebSocket RPC server mainloop in the background
        let rpc_clone = rpc_server.clone();
        let websocket_server_task = tokio::spawn(async move {
            tracing::info!("Starting WebSocket RPC server...");
            BaseChannel::with_defaults(server_chan)
                .execute(rpc_clone.serve())
                .for_each(|resp| async move {
                    tokio::spawn(resp);
                })
                .await;
            tracing::info!("WebSocket RPC server stopped");
        });

        // // Spawn the TCP server for sidecar proxies in the background
        // let tcp_proxy_manager = proxy_manager.clone();
        // let tcp_server_task = tokio::spawn(async move {
        //     if let Err(e) = tcp_proxy_manager.start_tcp_server().await {
        //         tracing::error!("TCP server failed: {}", e);
        //     }
        // });

        // Report cluster info immediately after connection
        if let Err(e) = rpc_server.report_cluster_info().await {
            tracing::error!("Failed to report cluster info: {}", e);
            // Don't fail the entire connection for this, just log and continue
        } else {
            tracing::info!("Successfully reported cluster info");
        }

        let heartbeat_task = Self::spawn_cluster_heartbeat_task(rpc_client.clone());

        let websocket_result = websocket_server_task.await;

        self.watch_manager.clear_rpc_client().await;
        self.proxy_manager.clear_cluster_rpc_client().await;
        heartbeat_task.abort();
        let _ = heartbeat_task.await;

        if let Err(e) = websocket_result {
            return Err(anyhow!("WebSocket RPC server task failed: {}", e));
        }

        Ok(())
    }

    async fn new_kube_client() -> Result<kube::Client> {
        let config = Self::retrieve_cluster_config().await?;
        let client = kube::Client::try_from(config)?;
        Ok(client)
    }

    async fn retrieve_cluster_config() -> Result<kube::Config> {
        Ok(kube::Config::infer().await?)
    }

    fn detect_manager_namespace() -> Option<String> {
        if let Ok(ns) = std::env::var("POD_NAMESPACE") {
            let trimmed = ns.trim();
            if !trimmed.is_empty() {
                return Some(trimmed.to_string());
            }
        }

        fs::read_to_string("/var/run/secrets/kubernetes.io/serviceaccount/namespace")
            .ok()
            .map(|contents| contents.trim().to_string())
            .filter(|value| !value.is_empty())
    }
}

impl KubeManager {
    fn spawn_cluster_heartbeat_task(rpc_client: KubeClusterRpcClient) -> JoinHandle<()> {
        tokio::spawn(async move {
            let mut interval =
                tokio::time::interval(Duration::from_secs(CLUSTER_HEARTBEAT_INTERVAL_SECS));
            interval.set_missed_tick_behavior(MissedTickBehavior::Skip);

            if !Self::send_cluster_heartbeat(&rpc_client).await {
                return;
            }

            loop {
                interval.tick().await;
                if !Self::send_cluster_heartbeat(&rpc_client).await {
                    break;
                }
            }
        })
    }

    async fn send_cluster_heartbeat(rpc_client: &KubeClusterRpcClient) -> bool {
        match rpc_client.heartbeat(tarpc::context::current()).await {
            Ok(Ok(())) => true,
            Ok(Err(err)) => {
                tracing::warn!("Cluster heartbeat rejected by API: {}", err);
                false
            }
            Err(err) => {
                tracing::debug!("Cluster heartbeat stopped: {}", err);
                false
            }
        }
    }

    pub(crate) async fn collect_cluster_info(&self) -> Result<KubeClusterInfo> {
        let client = &self.kube_client;

        // Get cluster version
        let version = client.apiserver_version().await?;

        // Get nodes and calculate cluster resources
        let nodes = self.get_cluster_nodes(client).await?;
        let (total_cpu_millicores, total_memory_bytes) = self.calculate_cluster_resources(&nodes);
        let node_count = nodes.len() as u32;
        let status = KubeClusterStatus::Ready;

        // Detect provider and region from node labels
        let (detected_provider, detected_region) = self.detect_provider_and_region(&nodes);

        // Use detected values with environment variable fallbacks
        let provider = detected_provider.or_else(|| std::env::var("CLUSTER_PROVIDER").ok());
        let region = detected_region.or_else(|| std::env::var("CLUSTER_REGION").ok());

        Ok(KubeClusterInfo {
            cluster_version: format!("{}.{}", version.major, version.minor),
            node_count,
            available_cpu: format!("{}m", total_cpu_millicores),
            available_memory: format!("{}bytes", total_memory_bytes),
            provider,
            region,
            status,
            manager_namespace: self.manager_namespace.clone(),
        })
    }

    async fn get_cluster_nodes(
        &self,
        client: &kube::Client,
    ) -> Result<Vec<k8s_openapi::api::core::v1::Node>> {
        let nodes: kube::Api<k8s_openapi::api::core::v1::Node> = kube::Api::all(client.clone());
        let node_list = nodes.list(&ListParams::default()).await?;
        Ok(node_list.items)
    }

    fn calculate_cluster_resources(
        &self,
        nodes: &[k8s_openapi::api::core::v1::Node],
    ) -> (i64, i64) {
        let mut total_cpu_millicores = 0i64;
        let mut total_memory_bytes = 0i64;

        for node in nodes {
            if let Some(status) = &node.status {
                if let Some(allocatable) = &status.allocatable {
                    total_cpu_millicores += Self::parse_cpu_resource(allocatable.get("cpu"));
                    total_memory_bytes += Self::parse_memory_resource(allocatable.get("memory"));
                }
            }
        }

        (total_cpu_millicores, total_memory_bytes)
    }

    fn parse_cpu_resource(
        cpu_quantity: Option<&k8s_openapi::apimachinery::pkg::api::resource::Quantity>,
    ) -> i64 {
        if let Some(cpu) = cpu_quantity {
            let cpu_str = cpu.0.as_str();
            if cpu_str.ends_with('m') {
                cpu_str.trim_end_matches('m').parse::<i64>().unwrap_or(0)
            } else {
                cpu_str.parse::<i64>().unwrap_or(0) * 1000
            }
        } else {
            0
        }
    }

    pub(crate) async fn configure_namespace_watches(&self, namespaces: Vec<String>) -> Result<()> {
        tracing::info!(
            namespace_count = namespaces.len(),
            "Configuring namespace watches"
        );
        self.watch_manager.configure_watches(namespaces).await
    }

    pub(crate) async fn add_namespace_watch(&self, namespace: String) -> Result<()> {
        tracing::info!(namespace = namespace.as_str(), "Adding namespace watch");
        self.watch_manager.add_namespace_watch(namespace).await
    }

    pub(crate) async fn remove_namespace_watch(&self, namespace: String) -> Result<()> {
        tracing::info!(namespace = namespace.as_str(), "Removing namespace watch");
        self.watch_manager.remove_namespace_watch(namespace).await
    }

    fn parse_memory_resource(
        memory_quantity: Option<&k8s_openapi::apimachinery::pkg::api::resource::Quantity>,
    ) -> i64 {
        if let Some(memory) = memory_quantity {
            let memory_str = memory.0.as_str();
            if memory_str.ends_with("Ki") {
                memory_str
                    .trim_end_matches("Ki")
                    .parse::<i64>()
                    .unwrap_or(0)
                    * 1024
            } else if memory_str.ends_with("Mi") {
                memory_str
                    .trim_end_matches("Mi")
                    .parse::<i64>()
                    .unwrap_or(0)
                    * 1024
                    * 1024
            } else if memory_str.ends_with("Gi") {
                memory_str
                    .trim_end_matches("Gi")
                    .parse::<i64>()
                    .unwrap_or(0)
                    * 1024
                    * 1024
                    * 1024
            } else {
                0
            }
        } else {
            0
        }
    }

    fn detect_provider_and_region(
        &self,
        nodes: &[k8s_openapi::api::core::v1::Node],
    ) -> (Option<String>, Option<String>) {
        let mut detected_provider = nodes
            .iter()
            .filter_map(|node| {
                node.spec
                    .as_ref()
                    .and_then(|spec| spec.provider_id.as_deref())
                    .and_then(Self::detect_provider_from_provider_id)
            })
            .next();
        let mut detected_region = None;

        for node in nodes {
            if let Some(labels) = &node.metadata.labels {
                // Get region from topology labels
                if detected_region.is_none() {
                    detected_region = labels
                        .get("topology.kubernetes.io/region")
                        .or_else(|| labels.get("failure-domain.beta.kubernetes.io/region"))
                        .cloned();
                }

                // Detect provider from instance type
                if detected_provider.is_none() {
                    detected_provider = Self::detect_provider_from_labels(labels).or_else(|| {
                        labels
                            .get("node.kubernetes.io/instance-type")
                            .and_then(|value| Self::detect_provider_from_instance_type(value))
                    });
                }

                // Break early if we have both
                if detected_provider.is_some() && detected_region.is_some() {
                    break;
                }
            }
        }

        (detected_provider, detected_region)
    }

    fn detect_provider_from_provider_id(provider_id: &str) -> Option<String> {
        if provider_id.starts_with("aws:///") {
            Some("AWS".to_string())
        } else if provider_id.starts_with("gce://") {
            Some("GCP".to_string())
        } else if provider_id.starts_with("azure:///") {
            Some("Azure".to_string())
        } else {
            None
        }
    }

    fn detect_provider_from_instance_type(instance_type: &str) -> Option<String> {
        if instance_type.starts_with('m')
            || instance_type.starts_with('c')
            || instance_type.starts_with('r')
            || instance_type.starts_with('t')
            || instance_type.starts_with('a')
            || instance_type.starts_with('i')
        {
            Some("AWS".to_string())
        } else if instance_type.starts_with('n')
            || instance_type.starts_with('e')
            || instance_type.contains("standard")
        {
            Some("GCP".to_string())
        } else if instance_type.starts_with("Standard_") {
            Some("Azure".to_string())
        } else {
            None
        }
    }

    fn detect_provider_from_labels(
        labels: &std::collections::BTreeMap<String, String>,
    ) -> Option<String> {
        if labels.contains_key("eks.amazonaws.com/nodegroup")
            || labels.contains_key("alpha.eksctl.io/nodegroup-name")
            || labels.contains_key("karpenter.sh/nodepool")
        {
            Some("AWS".to_string())
        } else if labels.contains_key("cloud.google.com/gke-nodepool")
            || labels.contains_key("cloud.google.com/machine-family")
            || labels.contains_key("topology.gke.io/zone")
        {
            Some("GCP".to_string())
        } else if labels.contains_key("kubernetes.azure.com/nodepool-name")
            || labels.contains_key("agentpool")
            || labels.contains_key("azure.microsoft.com/machine")
        {
            Some("Azure".to_string())
        } else {
            None
        }
    }

    pub(crate) async fn collect_workloads(
        &self,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList> {
        tracing::debug!("pagination is {pagination:?}");

        let (cursor, limit) = if let Some(pagination) = pagination {
            (pagination.cursor, pagination.limit.max(1))
        } else {
            (None, 50) // Default reasonable limit
        };

        tracing::debug!("cursor {cursor:?}");

        // Collect workloads sequentially by resource type, starting from the cursor position
        let mut all_workloads = Vec::new();
        let all_resource_types = [
            KubeWorkloadKind::Deployment,
            KubeWorkloadKind::StatefulSet,
            KubeWorkloadKind::DaemonSet,
            KubeWorkloadKind::ReplicaSet,
            KubeWorkloadKind::Pod,
            KubeWorkloadKind::Job,
            KubeWorkloadKind::CronJob,
        ];

        // Filter resource types based on workload_kind_filter
        let resource_types: Vec<KubeWorkloadKind> = if let Some(filter_kind) = workload_kind_filter
        {
            vec![filter_kind]
        } else {
            all_resource_types.to_vec()
        };

        // Find starting resource type index
        let start_index = if let Some(cursor) = &cursor {
            resource_types
                .iter()
                .position(|rt| *rt == cursor.workload_kind)
                .unwrap_or(0)
        } else {
            0
        };

        let mut resource_types_iter = resource_types.iter().skip(start_index).peekable();
        while let Some(current_resource_type) = resource_types_iter.next() {
            // Use cursor only for the first resource type we're collecting from
            let current_cursor = if let Some(cursor) = &cursor {
                if *current_resource_type == cursor.workload_kind {
                    cursor.continue_token.clone()
                } else {
                    None
                }
            } else {
                None
            };
            let collect_limit = limit.saturating_sub(all_workloads.len());

            let (collected_workloads, resource_continue_token) = match *current_resource_type {
                KubeWorkloadKind::Deployment => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::deployment_to_workload,
                    )
                    .await?
                }
                KubeWorkloadKind::StatefulSet => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::statefulset_to_workload,
                    )
                    .await?
                }
                KubeWorkloadKind::DaemonSet => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::daemonset_to_workload,
                    )
                    .await?
                }
                KubeWorkloadKind::ReplicaSet => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::replicaset_to_workload,
                    )
                    .await?
                }
                KubeWorkloadKind::Pod => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::pod_to_workload,
                    )
                    .await?
                }
                KubeWorkloadKind::Job => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::job_to_workload,
                    )
                    .await?
                }
                KubeWorkloadKind::CronJob => {
                    self.collect_resources_with_cursor(
                        namespace.as_deref(),
                        current_cursor,
                        collect_limit,
                        include_system_workloads,
                        Self::cronjob_to_workload,
                    )
                    .await?
                }
            };

            all_workloads.extend(collected_workloads);
            if all_workloads.len() >= limit {
                let next_cursor = if let Some(continue_token) = resource_continue_token {
                    // If we have a continue token for this resource type,
                    // we know that we filled our limit,
                    // so we are done for the collection
                    // and we should use this token for the next request
                    Some(PaginationCursor {
                        workload_kind: current_resource_type.clone(),
                        continue_token: Some(continue_token),
                    })
                } else {
                    // if we don't have a continue token
                    // we know that we had everything from this current resource type
                    // so we need to set the cursor to the next workload kind
                    resource_types_iter.peek().map(|next| PaginationCursor {
                        workload_kind: (**next).clone(),
                        continue_token: None,
                    })
                };
                return Ok(KubeWorkloadList {
                    workloads: all_workloads,
                    next_cursor,
                });
            }
        }

        // at this point, we should already iter over all the resources we can find
        // so we know there no more
        Ok(KubeWorkloadList {
            workloads: all_workloads,
            next_cursor: None,
        })
    }

    pub(crate) async fn collect_namespaces(&self) -> Result<Vec<KubeNamespaceInfo>> {
        let client = &self.kube_client;
        let namespaces: kube::Api<Namespace> = kube::Api::all((**client).clone());

        let namespace_list = namespaces.list(&ListParams::default()).await?;

        let mut kube_namespaces = Vec::new();
        for namespace in namespace_list.items {
            let name = namespace.metadata.name.unwrap_or_default();
            let status = namespace
                .status
                .as_ref()
                .and_then(|s| s.phase.as_ref())
                .map(|p| p.clone())
                .unwrap_or_else(|| "Unknown".to_string());
            let created_at = namespace
                .metadata
                .creation_timestamp
                .map(|t| format!("{t:?}"));

            kube_namespaces.push(KubeNamespaceInfo {
                name,
                status,
                created_at,
            });
        }

        kube_namespaces.sort_by(|a, b| a.name.cmp(&b.name));

        Ok(kube_namespaces)
    }

    // Generic helper method for collecting resources with cursor-based pagination
    // Only includes top-level resources (those without owner references)
    async fn collect_resources_with_cursor<T, F>(
        &self,
        namespace: Option<&str>,
        cursor: Option<String>,
        limit: usize,
        include_system_workloads: bool,
        converter: F,
    ) -> Result<(Vec<KubeWorkload>, Option<String>)>
    where
        T: kube::Resource<Scope = NamespaceResourceScope>
            + Clone
            + serde::de::DeserializeOwned
            + std::fmt::Debug,
        T::DynamicType: Default,
        F: Fn(T) -> KubeWorkload,
    {
        let client = &self.kube_client;
        let api: kube::Api<T> = if let Some(ns) = namespace {
            kube::Api::namespaced((**client).clone(), ns)
        } else {
            kube::Api::all((**client).clone())
        };

        let batch_size = 50;
        let mut all_workloads = Vec::new();
        let mut continue_token = cursor;

        loop {
            let list_params = ListParams {
                limit: Some(batch_size),
                continue_token: continue_token.clone(),
                ..Default::default()
            };

            let list = api.list(&list_params).await?;
            for item in list.items {
                // Only include top-level resources (no owner references)
                if !Self::has_owner_references(&item) {
                    let is_system_workload = Self::is_system_workload(&item);

                    // Include workload based on system workloads filter
                    if include_system_workloads || !is_system_workload {
                        let workload = converter(item);
                        all_workloads.push(workload);
                    }
                }
            }

            continue_token = list.metadata.continue_;
            if continue_token.is_none() || all_workloads.len() >= limit {
                break;
            }
        }

        Ok((all_workloads, continue_token))
    }

    fn deployment_to_workload(deployment: Deployment) -> KubeWorkload {
        let status = if deployment
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0)
            > 0
        {
            KubeWorkloadStatus::Running
        } else {
            KubeWorkloadStatus::Pending
        };

        KubeWorkload {
            name: deployment.metadata.name.unwrap_or_default(),
            namespace: deployment.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::Deployment,
            replicas: deployment.spec.as_ref().and_then(|s| s.replicas),
            ready_replicas: deployment.status.as_ref().and_then(|s| s.ready_replicas),
            status,
            created_at: deployment
                .metadata
                .creation_timestamp
                .map(|t| format!("{t:?}")),
            labels: deployment
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    fn statefulset_to_workload(statefulset: StatefulSet) -> KubeWorkload {
        let status = if statefulset
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0)
            > 0
        {
            KubeWorkloadStatus::Running
        } else {
            KubeWorkloadStatus::Pending
        };

        KubeWorkload {
            name: statefulset.metadata.name.unwrap_or_default(),
            namespace: statefulset.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::StatefulSet,
            replicas: statefulset.spec.as_ref().and_then(|s| s.replicas),
            ready_replicas: statefulset.status.as_ref().and_then(|s| s.ready_replicas),
            status,
            created_at: statefulset
                .metadata
                .creation_timestamp
                .map(|t| format!("{t:?}")),
            labels: statefulset
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    fn daemonset_to_workload(daemonset: DaemonSet) -> KubeWorkload {
        let status = if daemonset
            .status
            .as_ref()
            .map(|s| s.number_ready)
            .unwrap_or(0)
            > 0
        {
            KubeWorkloadStatus::Running
        } else {
            KubeWorkloadStatus::Pending
        };

        KubeWorkload {
            name: daemonset.metadata.name.unwrap_or_default(),
            namespace: daemonset.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::DaemonSet,
            replicas: daemonset
                .status
                .as_ref()
                .map(|s| s.desired_number_scheduled),
            ready_replicas: daemonset.status.as_ref().map(|s| s.number_ready),
            status,
            created_at: daemonset
                .metadata
                .creation_timestamp
                .map(|t| format!("{t:?}")),
            labels: daemonset
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    fn pod_to_workload(pod: Pod) -> KubeWorkload {
        let status = match pod.status.as_ref().and_then(|s| s.phase.as_deref()) {
            Some("Running") => KubeWorkloadStatus::Running,
            Some("Pending") => KubeWorkloadStatus::Pending,
            Some("Failed") => KubeWorkloadStatus::Failed,
            Some("Succeeded") => KubeWorkloadStatus::Succeeded,
            _ => KubeWorkloadStatus::Unknown,
        };

        KubeWorkload {
            name: pod.metadata.name.unwrap_or_default(),
            namespace: pod.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::Pod,
            replicas: Some(1),
            ready_replicas: if status == KubeWorkloadStatus::Running {
                Some(1)
            } else {
                Some(0)
            },
            status,
            created_at: pod.metadata.creation_timestamp.map(|t| format!("{:?}", t)),
            labels: pod
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    // Generic helper function to check if a resource has owner references (not top-level)
    fn has_owner_references<T>(resource: &T) -> bool
    where
        T: kube::Resource,
    {
        resource
            .meta()
            .owner_references
            .as_ref()
            .is_some_and(|refs| !refs.is_empty())
    }

    // Helper function to check if a workload is a system workload
    fn is_system_workload<T>(resource: &T) -> bool
    where
        T: kube::Resource,
    {
        let meta = resource.meta();

        // Check if in kube-system namespace
        if let Some(namespace) = &meta.namespace {
            return namespace == "kube-system";
        }

        false
    }

    fn job_to_workload(job: Job) -> KubeWorkload {
        let status = if job.status.as_ref().and_then(|s| s.succeeded).unwrap_or(0) > 0 {
            KubeWorkloadStatus::Succeeded
        } else if job.status.as_ref().and_then(|s| s.failed).unwrap_or(0) > 0 {
            KubeWorkloadStatus::Failed
        } else {
            KubeWorkloadStatus::Running
        };

        KubeWorkload {
            name: job.metadata.name.unwrap_or_default(),
            namespace: job.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::Job,
            replicas: job.spec.as_ref().and_then(|s| s.parallelism),
            ready_replicas: job.status.as_ref().and_then(|s| s.succeeded),
            status,
            created_at: job.metadata.creation_timestamp.map(|t| format!("{t:?}")),
            labels: job
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    fn replicaset_to_workload(replicaset: ReplicaSet) -> KubeWorkload {
        let status = if replicaset
            .status
            .as_ref()
            .and_then(|s| s.ready_replicas)
            .unwrap_or(0)
            > 0
        {
            KubeWorkloadStatus::Running
        } else {
            KubeWorkloadStatus::Pending
        };

        KubeWorkload {
            name: replicaset.metadata.name.unwrap_or_default(),
            namespace: replicaset.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::ReplicaSet,
            replicas: replicaset.spec.as_ref().and_then(|s| s.replicas),
            ready_replicas: replicaset.status.as_ref().and_then(|s| s.ready_replicas),
            status,
            created_at: replicaset
                .metadata
                .creation_timestamp
                .map(|t| format!("{t:?}")),
            labels: replicaset
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    fn cronjob_to_workload(cronjob: CronJob) -> KubeWorkload {
        let status = if cronjob.status.as_ref().is_some() {
            KubeWorkloadStatus::Running
        } else {
            KubeWorkloadStatus::Pending
        };

        KubeWorkload {
            name: cronjob.metadata.name.unwrap_or_default(),
            namespace: cronjob.metadata.namespace.unwrap_or_default(),
            kind: KubeWorkloadKind::CronJob,
            replicas: Some(1), // CronJobs don't have replicas concept
            ready_replicas: Some(1),
            status,
            created_at: cronjob
                .metadata
                .creation_timestamp
                .map(|t| format!("{t:?}")),
            labels: cronjob
                .metadata
                .labels
                .unwrap_or_default()
                .into_iter()
                .collect(),
        }
    }

    pub(crate) async fn get_workload_by_name(
        &self,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>> {
        let client = &self.kube_client;

        // Try to find the workload by checking different resource types
        // Check Deployment first
        let deployments: kube::Api<Deployment> =
            kube::Api::namespaced((**client).clone(), &namespace);
        if let Ok(deployment) = deployments.get(&name).await {
            return Ok(Some(Self::deployment_to_workload(deployment)));
        }

        // Check StatefulSet
        let statefulsets: kube::Api<StatefulSet> =
            kube::Api::namespaced((**client).clone(), &namespace);
        if let Ok(statefulset) = statefulsets.get(&name).await {
            return Ok(Some(Self::statefulset_to_workload(statefulset)));
        }

        // Check DaemonSet
        let daemonsets: kube::Api<DaemonSet> =
            kube::Api::namespaced((**client).clone(), &namespace);
        if let Ok(daemonset) = daemonsets.get(&name).await {
            return Ok(Some(Self::daemonset_to_workload(daemonset)));
        }

        // Check Pod
        let pods: kube::Api<Pod> = kube::Api::namespaced((**client).clone(), &namespace);
        if let Ok(pod) = pods.get(&name).await {
            return Ok(Some(Self::pod_to_workload(pod)));
        }

        // Check Job
        let jobs: kube::Api<Job> = kube::Api::namespaced((**client).clone(), &namespace);
        if let Ok(job) = jobs.get(&name).await {
            return Ok(Some(Self::job_to_workload(job)));
        }

        Ok(None)
    }

    pub(crate) async fn fetch_namespaced_resources(
        &self,
        requests: Vec<NamespacedResourceRequest>,
    ) -> Result<Vec<NamespacedResourceResponse>> {
        let client = &self.kube_client;
        let mut results = Vec::with_capacity(requests.len());

        for request in requests {
            let namespace = request.namespace.clone();
            let configmaps_api: kube::Api<ConfigMap> =
                kube::Api::namespaced((**client).clone(), &namespace);
            let secrets_api: kube::Api<Secret> =
                kube::Api::namespaced((**client).clone(), &namespace);

            let mut configmap_yamls = HashMap::new();
            for name in request.configmaps {
                match configmaps_api.get(&name).await {
                    Ok(configmap) => {
                        if let Ok(yaml) = serde_yaml::to_string(&configmap) {
                            configmap_yamls.insert(name.clone(), yaml);
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Could not fetch ConfigMap {}/{}: {}", namespace, name, err);
                    }
                }
            }

            let mut secret_yamls = HashMap::new();
            for name in request.secrets {
                match secrets_api.get(&name).await {
                    Ok(secret) => {
                        if let Ok(yaml) = serde_yaml::to_string(&secret) {
                            secret_yamls.insert(name.clone(), yaml);
                        }
                    }
                    Err(err) => {
                        tracing::warn!("Could not fetch Secret {}/{}: {}", namespace, name, err);
                    }
                }
            }

            results.push(NamespacedResourceResponse {
                namespace,
                configmaps: configmap_yamls,
                secrets: secret_yamls,
            });
        }

        Ok(results)
    }

    pub(crate) async fn apply_workloads_with_resources(
        &self,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let client = &self.kube_client;

        // Step 1: Ensure namespace exists
        self.ensure_namespace_exists(&namespace).await?;

        // Step 2: Apply shared resources first (configmaps, secrets, then services)
        // Apply configmaps first as they might be referenced by workloads
        for (name, configmap_yaml) in &workloads_with_resources.configmaps {
            tracing::info!("Applying ConfigMap '{}' to namespace '{}'", name, namespace);
            self.apply_single_configmap(client, &namespace, configmap_yaml, &labels)
                .await?;
        }

        // Apply secrets next as they might be referenced by workloads
        for (name, secret_yaml) in &workloads_with_resources.secrets {
            tracing::info!("Applying Secret '{}' to namespace '{}'", name, namespace);
            self.apply_single_secret(client, &namespace, secret_yaml, &labels)
                .await?;
        }

        // Step 3: Apply all workloads
        for workload in &workloads_with_resources.workloads {
            tracing::info!("Applying workload to namespace '{}'", namespace);
            self.apply_workload_only(client, &namespace, workload, &labels)
                .await?;
        }

        // Step 4: Apply services last as they reference workloads
        for (name, service_with_yaml) in &workloads_with_resources.services {
            tracing::info!("Applying Service '{}' to namespace '{}'", name, namespace);
            self.apply_single_service(client, &namespace, &service_with_yaml.yaml, &labels)
                .await?;
        }

        tracing::info!(
            "Successfully applied all workloads and resources to namespace '{}'",
            namespace
        );
        Ok(())
    }

    async fn apply_single_configmap(
        &self,
        client: &kube::Client,
        namespace: &str,
        configmap_yaml: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut configmap: ConfigMap = serde_yaml::from_str(configmap_yaml)?;

        // Add environment labels to configmap
        self.add_labels_to_metadata(&mut configmap.metadata, labels);

        // Force namespace
        configmap.metadata.namespace = Some(namespace.to_string());

        let configmaps_api: kube::Api<ConfigMap> =
            kube::Api::namespaced((*client).clone(), namespace);

        match configmaps_api
            .get_opt(&configmap.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                configmaps_api
                    .replace(
                        &configmap.metadata.name.as_ref().unwrap(),
                        &Default::default(),
                        &configmap,
                    )
                    .await?;
                tracing::info!(
                    "Updated configmap: {}",
                    configmap.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                configmaps_api
                    .create(&Default::default(), &configmap)
                    .await?;
                tracing::info!(
                    "Created configmap: {}",
                    configmap.metadata.name.as_ref().unwrap()
                );
            }
        }
        Ok(())
    }

    async fn apply_single_secret(
        &self,
        client: &kube::Client,
        namespace: &str,
        secret_yaml: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut secret: Secret = serde_yaml::from_str(secret_yaml)?;

        // Add environment labels to secret
        self.add_labels_to_metadata(&mut secret.metadata, labels);

        // Force namespace
        secret.metadata.namespace = Some(namespace.to_string());

        let secrets_api: kube::Api<Secret> = kube::Api::namespaced((*client).clone(), namespace);

        match secrets_api
            .get_opt(&secret.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                secrets_api
                    .replace(
                        &secret.metadata.name.as_ref().unwrap(),
                        &Default::default(),
                        &secret,
                    )
                    .await?;
                tracing::info!("Updated secret: {}", secret.metadata.name.as_ref().unwrap());
            }
            None => {
                secrets_api.create(&Default::default(), &secret).await?;
                tracing::info!("Created secret: {}", secret.metadata.name.as_ref().unwrap());
            }
        }
        Ok(())
    }

    async fn apply_single_service(
        &self,
        client: &kube::Client,
        namespace: &str,
        service_yaml: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut service: Service = serde_yaml::from_str(service_yaml)?;

        // Add environment labels to service
        self.add_labels_to_metadata(&mut service.metadata, labels);

        // Force namespace
        service.metadata.namespace = Some(namespace.to_string());

        let services_api: kube::Api<Service> = kube::Api::namespaced((*client).clone(), namespace);

        match services_api
            .get_opt(&service.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                services_api
                    .replace(
                        &service.metadata.name.as_ref().unwrap(),
                        &Default::default(),
                        &service,
                    )
                    .await?;
                tracing::info!(
                    "Updated service: {}",
                    service.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                services_api.create(&Default::default(), &service).await?;
                tracing::info!(
                    "Created service: {}",
                    service.metadata.name.as_ref().unwrap()
                );
            }
        }
        Ok(())
    }

    async fn apply_workload_only(
        &self,
        client: &kube::Client,
        namespace: &str,
        workload: &KubeWorkloadYamlOnly,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        match workload {
            KubeWorkloadYamlOnly::Deployment(yaml) => {
                self.apply_deployment(namespace, yaml, labels).await?;
            }
            KubeWorkloadYamlOnly::StatefulSet(yaml) => {
                self.apply_statefulset(client, namespace, yaml, labels)
                    .await?;
            }
            KubeWorkloadYamlOnly::DaemonSet(yaml) => {
                self.apply_daemonset(client, namespace, yaml, labels)
                    .await?;
            }
            KubeWorkloadYamlOnly::ReplicaSet(yaml) => {
                self.apply_replicaset(client, namespace, yaml, labels)
                    .await?;
            }
            KubeWorkloadYamlOnly::Pod(yaml) => {
                self.apply_pod(client, namespace, yaml, labels).await?;
            }
            KubeWorkloadYamlOnly::Job(yaml) => {
                self.apply_job(client, namespace, yaml, labels).await?;
            }
            KubeWorkloadYamlOnly::CronJob(yaml) => {
                self.apply_cronjob(client, namespace, yaml, labels).await?;
            }
        }
        Ok(())
    }

    async fn ensure_namespace_exists(&self, namespace: &str) -> Result<()> {
        let client = &self.kube_client;

        let namespaces: kube::Api<Namespace> = kube::Api::all((**client).clone());

        // Check if namespace exists
        if namespaces.get_opt(namespace).await?.is_some() {
            return Ok(());
        }

        // Create namespace if it doesn't exist
        let ns = Namespace {
            metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
                name: Some(namespace.to_string()),
                ..Default::default()
            },
            ..Default::default()
        };

        namespaces.create(&Default::default(), &ns).await?;
        tracing::info!("Created namespace: {}", namespace);
        Ok(())
    }

    pub(crate) async fn destroy_environment(
        &self,
        environment_id: Uuid,
        namespace: &str,
    ) -> Result<()> {
        tracing::info!(
            "Destroying environment {} and cleaning namespace '{}'",
            environment_id,
            namespace
        );
        self.delete_namespace(namespace).await
    }

    async fn delete_namespace(&self, namespace: &str) -> Result<()> {
        let client = &self.kube_client;
        let namespaces: kube::Api<Namespace> = kube::Api::all((**client).clone());

        if namespaces.get_opt(namespace).await?.is_none() {
            tracing::info!(
                "Namespace '{}' not found during deletion, assuming already removed",
                namespace
            );
            return Ok(());
        }

        match namespaces.delete(namespace, &DeleteParams::default()).await {
            Ok(_) => {
                let timeout = Duration::from_secs(60);
                let start = Instant::now();
                loop {
                    if namespaces.get_opt(namespace).await?.is_none() {
                        tracing::info!("Namespace '{}' deleted successfully", namespace);
                        break;
                    }

                    if start.elapsed() >= timeout {
                        return Err(anyhow!(
                            "Timed out waiting for namespace '{}' deletion",
                            namespace
                        ));
                    }

                    sleep(Duration::from_secs(1)).await;
                }
            }
            Err(kube::Error::Api(ae)) if ae.code == 404 => {
                tracing::info!("Namespace '{}' already deleted", namespace);
            }
            Err(e) => {
                return Err(anyhow!("Failed to delete namespace '{}': {}", namespace, e));
            }
        }

        Ok(())
    }

    async fn apply_deployment(
        &self,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut deployment: Deployment = serde_yaml::from_str(yaml_manifest)?;

        // Add environment labels to deployment
        self.add_labels_to_metadata(&mut deployment.metadata, labels);

        // Force namespace
        deployment.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<Deployment> =
            kube::Api::namespaced((*self.kube_client).clone(), namespace);

        // Try to create or update the deployment
        match api
            .get_opt(&deployment.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                // Update existing deployment
                api.replace(
                    &deployment.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &deployment,
                )
                .await?;
                tracing::info!(
                    "Updated deployment: {}",
                    deployment.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                // Create new deployment
                api.create(&Default::default(), &deployment).await?;
                tracing::info!(
                    "Created deployment: {}",
                    deployment.metadata.name.as_ref().unwrap()
                );
            }
        }

        Ok(())
    }

    async fn apply_statefulset(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut statefulset: StatefulSet = serde_yaml::from_str(yaml_manifest)?;

        self.add_labels_to_metadata(&mut statefulset.metadata, labels);
        statefulset.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<StatefulSet> = kube::Api::namespaced((*client).clone(), namespace);

        match api
            .get_opt(&statefulset.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                api.replace(
                    &statefulset.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &statefulset,
                )
                .await?;
                tracing::info!(
                    "Updated statefulset: {}",
                    statefulset.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                api.create(&Default::default(), &statefulset).await?;
                tracing::info!(
                    "Created statefulset: {}",
                    statefulset.metadata.name.as_ref().unwrap()
                );
            }
        }

        Ok(())
    }

    async fn apply_daemonset(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut daemonset: DaemonSet = serde_yaml::from_str(yaml_manifest)?;

        self.add_labels_to_metadata(&mut daemonset.metadata, labels);
        daemonset.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<DaemonSet> = kube::Api::namespaced((*client).clone(), namespace);

        match api
            .get_opt(&daemonset.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                api.replace(
                    &daemonset.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &daemonset,
                )
                .await?;
                tracing::info!(
                    "Updated daemonset: {}",
                    daemonset.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                api.create(&Default::default(), &daemonset).await?;
                tracing::info!(
                    "Created daemonset: {}",
                    daemonset.metadata.name.as_ref().unwrap()
                );
            }
        }

        Ok(())
    }

    async fn apply_pod(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut pod: Pod = serde_yaml::from_str(yaml_manifest)?;

        self.add_labels_to_metadata(&mut pod.metadata, labels);
        pod.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<Pod> = kube::Api::namespaced((*client).clone(), namespace);

        match api.get_opt(&pod.metadata.name.as_ref().unwrap()).await? {
            Some(_) => {
                api.replace(
                    &pod.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &pod,
                )
                .await?;
                tracing::info!("Updated pod: {}", pod.metadata.name.as_ref().unwrap());
            }
            None => {
                api.create(&Default::default(), &pod).await?;
                tracing::info!("Created pod: {}", pod.metadata.name.as_ref().unwrap());
            }
        }

        Ok(())
    }

    async fn apply_job(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut job: Job = serde_yaml::from_str(yaml_manifest)?;

        self.add_labels_to_metadata(&mut job.metadata, labels);
        job.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<Job> = kube::Api::namespaced((*client).clone(), namespace);

        match api.get_opt(&job.metadata.name.as_ref().unwrap()).await? {
            Some(_) => {
                api.replace(
                    &job.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &job,
                )
                .await?;
                tracing::info!("Updated job: {}", job.metadata.name.as_ref().unwrap());
            }
            None => {
                api.create(&Default::default(), &job).await?;
                tracing::info!("Created job: {}", job.metadata.name.as_ref().unwrap());
            }
        }

        Ok(())
    }

    async fn apply_cronjob(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut cronjob: CronJob = serde_yaml::from_str(yaml_manifest)?;

        self.add_labels_to_metadata(&mut cronjob.metadata, labels);
        cronjob.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<CronJob> = kube::Api::namespaced((*client).clone(), namespace);

        match api
            .get_opt(&cronjob.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                api.replace(
                    &cronjob.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &cronjob,
                )
                .await?;
                tracing::info!(
                    "Updated cronjob: {}",
                    cronjob.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                api.create(&Default::default(), &cronjob).await?;
                tracing::info!(
                    "Created cronjob: {}",
                    cronjob.metadata.name.as_ref().unwrap()
                );
            }
        }

        Ok(())
    }

    async fn apply_replicaset(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut replicaset: ReplicaSet = serde_yaml::from_str(yaml_manifest)?;

        self.add_labels_to_metadata(&mut replicaset.metadata, labels);
        replicaset.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<ReplicaSet> = kube::Api::namespaced((*client).clone(), namespace);

        match api
            .get_opt(&replicaset.metadata.name.as_ref().unwrap())
            .await?
        {
            Some(_) => {
                api.replace(
                    &replicaset.metadata.name.as_ref().unwrap(),
                    &Default::default(),
                    &replicaset,
                )
                .await?;
                tracing::info!(
                    "Updated replicaset: {}",
                    replicaset.metadata.name.as_ref().unwrap()
                );
            }
            None => {
                api.create(&Default::default(), &replicaset).await?;
                tracing::info!(
                    "Created replicaset: {}",
                    replicaset.metadata.name.as_ref().unwrap()
                );
            }
        }

        Ok(())
    }

    fn add_labels_to_metadata(
        &self,
        metadata: &mut k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
        new_labels: &std::collections::HashMap<String, String>,
    ) {
        let labels = metadata.labels.get_or_insert_with(Default::default);
        for (key, value) in new_labels {
            labels.insert(key.clone(), value.clone());
        }
    }

    pub(crate) async fn get_raw_workload_yaml(
        &self,
        name: &str,
        namespace: &str,
        kind: &KubeWorkloadKind,
    ) -> Result<String> {
        let client = &self.kube_client;
        match kind {
            KubeWorkloadKind::Deployment => {
                let api: kube::Api<Deployment> =
                    kube::Api::namespaced((**client).clone(), namespace);
                let deployment = api.get(name).await?;
                Ok(serde_yaml::to_string(&deployment)?)
            }
            KubeWorkloadKind::StatefulSet => {
                let api: kube::Api<StatefulSet> =
                    kube::Api::namespaced((**client).clone(), namespace);
                let statefulset = api.get(name).await?;
                Ok(serde_yaml::to_string(&statefulset)?)
            }
            KubeWorkloadKind::DaemonSet => {
                let api: kube::Api<DaemonSet> =
                    kube::Api::namespaced((**client).clone(), namespace);
                let daemonset = api.get(name).await?;
                Ok(serde_yaml::to_string(&daemonset)?)
            }
            KubeWorkloadKind::ReplicaSet => {
                let api: kube::Api<ReplicaSet> =
                    kube::Api::namespaced((**client).clone(), namespace);
                let replicaset = api.get(name).await?;
                Ok(serde_yaml::to_string(&replicaset)?)
            }
            KubeWorkloadKind::Pod => {
                let api: kube::Api<Pod> = kube::Api::namespaced((**client).clone(), namespace);
                let pod = api.get(name).await?;
                Ok(serde_yaml::to_string(&pod)?)
            }
            KubeWorkloadKind::Job => {
                let api: kube::Api<Job> = kube::Api::namespaced((**client).clone(), namespace);
                let job = api.get(name).await?;
                Ok(serde_yaml::to_string(&job)?)
            }
            KubeWorkloadKind::CronJob => {
                let api: kube::Api<CronJob> = kube::Api::namespaced((**client).clone(), namespace);
                let cronjob = api.get(name).await?;
                Ok(serde_yaml::to_string(&cronjob)?)
            }
        }
    }

    pub async fn refresh_branch_service_routes(&self, environment_id: Uuid) -> Result<()> {
        self.proxy_manager
            .set_service_routes_if_registered(environment_id)
            .await
    }

    pub async fn get_devbox_direct_config(
        &self,
        user_id: Uuid,
        environment_id: Uuid,
        namespace: String,
        stun_observed_addr: Option<SocketAddr>,
    ) -> Result<Option<DirectTunnelConfig>, String> {
        let token = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + chrono::Duration::minutes(2);
        self.direct_endpoint
            .credential()
            .insert(token.clone(), expires_at)
            .await;

        let server_observed_addr = self.direct_endpoint.observed_addr();

        if let Some(addr) = stun_observed_addr {
            let direct_endpoint = self.direct_endpoint.clone();
            tokio::spawn(async move {
                match direct_endpoint.send_probe(addr).await {
                    Ok(()) => {
                        tracing::info!(
                            %addr,
                            "Sent hole-punch probe to devbox client before issuing config"
                        )
                    }
                    Err(err) => {
                        tracing::warn!(%addr, error = %err, "Failed to send probe to devbox client");
                    }
                }
            });
        }

        Ok(Some(DirectTunnelConfig {
            credential: DirectTunnelCredential { token, expires_at },
            server_certificate: Some(self.direct_endpoint.server_certificate().to_vec()),
            stun_observed_addr: server_observed_addr,
        }))
    }

    pub async fn set_devbox_routes(
        &self,
        environment_id: Uuid,
        routes: HashMap<Uuid, DevboxRouteConfig>,
    ) -> Result<(), String> {
        self.proxy_manager
            .set_devbox_routes(environment_id, routes)
            .await
    }

    pub async fn set_devbox_route(
        &self,
        environment_id: Uuid,
        route: DevboxRouteConfig,
    ) -> Result<(), String> {
        self.proxy_manager
            .set_devbox_route(environment_id, route)
            .await
    }

    pub async fn remove_devbox_route(
        &self,
        environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        self.proxy_manager
            .remove_devbox_route(environment_id, workload_id, branch_environment_id)
            .await
    }

    pub async fn update_branch_service_route(
        &self,
        base_environment_id: Uuid,
        workload_id: Uuid,
        route: ProxyBranchRouteConfig,
    ) -> Result<(), String> {
        self.proxy_manager
            .upsert_branch_service_route(base_environment_id, workload_id, route)
            .await
    }

    pub async fn remove_branch_service_route(
        &self,
        base_environment_id: Uuid,
        workload_id: Uuid,
        branch_environment_id: Uuid,
    ) -> Result<(), String> {
        self.proxy_manager
            .remove_branch_service_route(base_environment_id, workload_id, branch_environment_id)
            .await
    }

    pub async fn clear_devbox_routes(
        &self,
        environment_id: Uuid,
        branch_environment_id: Option<Uuid>,
    ) -> Result<(), String> {
        self.proxy_manager
            .clear_devbox_routes(environment_id, branch_environment_id)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

    // Helper function to create Quantity objects for testing

    fn create_quantity(value: &str) -> Quantity {
        Quantity(value.to_string())
    }

    #[test]
    fn test_parse_cpu_resource_millicores() {
        // Test millicores format
        let cpu_1000m = create_quantity("1000m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&cpu_1000m)), 1000);

        let cpu_500m = create_quantity("500m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&cpu_500m)), 500);

        let cpu_2500m = create_quantity("2500m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&cpu_2500m)), 2500);
    }

    #[test]
    fn test_parse_cpu_resource_cores() {
        // Test cores format (should be converted to millicores)
        let cpu_1 = create_quantity("1");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&cpu_1)), 1000);

        let cpu_2 = create_quantity("2");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&cpu_2)), 2000);

        let cpu_4 = create_quantity("4");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&cpu_4)), 4000);
    }

    #[test]
    fn test_parse_cpu_resource_edge_cases() {
        // Test None
        assert_eq!(KubeManager::parse_cpu_resource(None), 0);

        // Test invalid format
        let invalid_cpu = create_quantity("invalid");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&invalid_cpu)), 0);

        // Test empty string
        let empty_cpu = create_quantity("");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&empty_cpu)), 0);

        // Test zero values
        let zero_cpu = create_quantity("0");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&zero_cpu)), 0);

        let zero_cpu_m = create_quantity("0m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&zero_cpu_m)), 0);
    }

    #[test]
    fn test_parse_memory_resource_kibibytes() {
        // Test Ki format
        let mem_1024ki = create_quantity("1024Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_1024ki)),
            1024 * 1024
        );

        let mem_2048ki = create_quantity("2048Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_2048ki)),
            2048 * 1024
        );

        let mem_512ki = create_quantity("512Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_512ki)),
            512 * 1024
        );
    }

    #[test]
    fn test_parse_memory_resource_mebibytes() {
        // Test Mi format
        let mem_1mi = create_quantity("1Mi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_1mi)),
            1024 * 1024
        );

        let mem_256mi = create_quantity("256Mi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_256mi)),
            256 * 1024 * 1024
        );

        let mem_512mi = create_quantity("512Mi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_512mi)),
            512 * 1024 * 1024
        );
    }

    #[test]
    fn test_parse_memory_resource_gibibytes() {
        // Test Gi format
        let mem_1gi = create_quantity("1Gi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_1gi)),
            1024 * 1024 * 1024
        );

        let mem_2gi = create_quantity("2Gi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_2gi)),
            2 * 1024 * 1024 * 1024
        );

        let mem_8gi = create_quantity("8Gi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_8gi)),
            8 * 1024 * 1024 * 1024
        );
    }

    #[test]
    fn test_parse_memory_resource_edge_cases() {
        // Test None
        assert_eq!(KubeManager::parse_memory_resource(None), 0);

        // Test unsupported format (should return 0)
        let unsupported_mem = create_quantity("100bytes");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&unsupported_mem)),
            0
        );

        // Test invalid format
        let invalid_mem = create_quantity("invalid");
        assert_eq!(KubeManager::parse_memory_resource(Some(&invalid_mem)), 0);

        // Test empty string
        let empty_mem = create_quantity("");
        assert_eq!(KubeManager::parse_memory_resource(Some(&empty_mem)), 0);

        // Test zero values
        let zero_mem_ki = create_quantity("0Ki");
        assert_eq!(KubeManager::parse_memory_resource(Some(&zero_mem_ki)), 0);

        let zero_mem_mi = create_quantity("0Mi");
        assert_eq!(KubeManager::parse_memory_resource(Some(&zero_mem_mi)), 0);

        let zero_mem_gi = create_quantity("0Gi");
        assert_eq!(KubeManager::parse_memory_resource(Some(&zero_mem_gi)), 0);
    }

    #[test]
    fn test_parse_memory_resource_large_values() {
        // Test large realistic values
        let mem_7922180ki = create_quantity("7922180Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_7922180ki)),
            7922180 * 1024
        );

        let mem_16gi = create_quantity("16Gi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_16gi)),
            16 * 1024 * 1024 * 1024
        );

        let mem_32768mi = create_quantity("32768Mi");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&mem_32768mi)),
            32768 * 1024 * 1024
        );
    }

    #[test]
    fn test_parse_resources_real_world_examples() {
        // Common GKE node CPU allocations
        let gke_cpu_1 = create_quantity("940m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&gke_cpu_1)), 940);

        let gke_cpu_2 = create_quantity("1930m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&gke_cpu_2)), 1930);

        // Common GKE node memory allocations
        let gke_mem_1 = create_quantity("2702988Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&gke_mem_1)),
            2702988 * 1024
        );

        let gke_mem_2 = create_quantity("6601900Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&gke_mem_2)),
            6601900 * 1024
        );

        // AWS EKS examples
        let eks_cpu = create_quantity("1930m");
        assert_eq!(KubeManager::parse_cpu_resource(Some(&eks_cpu)), 1930);

        let eks_mem = create_quantity("3843684Ki");
        assert_eq!(
            KubeManager::parse_memory_resource(Some(&eks_mem)),
            3843684 * 1024
        );
    }
}
