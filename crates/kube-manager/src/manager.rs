use std::{collections::HashMap, sync::Arc};

use anyhow::{anyhow, Result};
use base64::{engine::general_purpose::STANDARD, Engine};
use futures::StreamExt;
use k8s_openapi::serde_json;
use k8s_openapi::{
    api::{
        apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
        batch::v1::{CronJob, Job},
        core::v1::{ConfigMap, Namespace, Pod, Secret, Service},
    },
    NamespaceResourceScope,
};
use kube::{api::ListParams, config::AuthInfo};
use lapdev_common::kube::{
    KubeAppCatalogWorkload, KubeClusterInfo, KubeClusterStatus, KubeContainerInfo, KubeNamespace,
    KubeWorkload, KubeWorkloadKind, KubeWorkloadList, KubeWorkloadStatus, PaginationCursor,
    PaginationParams, DEFAULT_KUBE_CLUSTER_URL, KUBE_CLUSTER_TOKEN_ENV_VAR,
    KUBE_CLUSTER_TOKEN_HEADER, KUBE_CLUSTER_URL_ENV_VAR,
};
use lapdev_kube_rpc::{
    KubeClusterRpcClient, KubeManagerRpc, KubeWorkloadWithServices, KubeWorkloadYaml,
    KubeWorkloadYamlOnly, KubeWorkloadsWithResources, WorkloadIdentifier,
};
use lapdev_rpc::spawn_twoway;
use serde::Deserialize;
use tarpc::server::{BaseChannel, Channel};
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_util::codec::LengthDelimitedCodec;

use crate::websocket_transport::WebSocketTransport;

const SCOPE: &[&str] = &["https://www.googleapis.com/auth/cloud-platform"];

#[derive(Clone)]
pub struct KubeManager {
    rpc_client: KubeClusterRpcClient,
    kube_client: Option<Arc<kube::Client>>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ListClustersResponse {
    pub clusters: Vec<Cluster>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ControlPlaneEndpointsConfig {
    pub ip_endpoints_config: IPEndpointsConfig,
    pub dns_endpoint_config: DNSEndpointConfig,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct IPEndpointsConfig {
    pub public_endpoint: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DNSEndpointConfig {
    pub endpoint: String,
    pub allow_external_traffic: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MasterAuth {
    pub cluster_ca_certificate: String,
    pub client_certificate: Option<String>,
    pub client_key: Option<String>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Cluster {
    pub name: String,
    pub description: Option<String>,
    pub locations: Vec<String>,
    pub endpoint: String,
    pub master_auth: MasterAuth,
    pub control_plane_endpoints_config: ControlPlaneEndpointsConfig,
}

impl KubeManager {
    pub async fn connect_cluster() -> Result<()> {
        let token = std::env::var(KUBE_CLUSTER_TOKEN_ENV_VAR)
            .map_err(|_| anyhow::anyhow!("can't find env var {}", KUBE_CLUSTER_TOKEN_ENV_VAR))?;
        let url = std::env::var(KUBE_CLUSTER_URL_ENV_VAR)
            .unwrap_or_else(|_| DEFAULT_KUBE_CLUSTER_URL.to_string());

        tracing::info!("Connecting to Lapdev cluster at: {}", url);

        let mut request = url.into_client_request()?;
        request
            .headers_mut()
            .insert(KUBE_CLUSTER_TOKEN_HEADER, token.parse()?);

        loop {
            match Self::handle_connection_cycle(request.clone()).await {
                Ok(_) => {}
                Err(e) => {
                    tracing::warn!("Connection cycle failed: {}, retrying in 5 seconds...", e);
                    tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                }
            }
        }
    }

    async fn handle_connection_cycle(
        request: tokio_tungstenite::tungstenite::http::Request<()>,
    ) -> Result<()> {
        tracing::info!("Attempting to connect to cluster...");

        let (stream, _) = tokio_tungstenite::connect_async(request.clone()).await?;

        tracing::info!("WebSocket connection established");

        let trans = WebSocketTransport::new(stream);
        let io = LengthDelimitedCodec::builder().new_framed(trans);

        let transport =
            tarpc::serde_transport::new(io, tarpc::tokio_serde::formats::Bincode::default());
        let (server_chan, client_chan, _) = spawn_twoway(transport);

        let rpc_client =
            KubeClusterRpcClient::new(tarpc::client::Config::default(), client_chan).spawn();

        let kube_client = Self::new_kube_client()
            .await
            .map_err(|e| {
                tracing::warn!("Failed to create Kubernetes client: {}", e);
                e
            })
            .ok();

        let rpc_server = KubeManager {
            rpc_client,
            kube_client: kube_client.map(Arc::new),
        };

        // Spawn the RPC server mainloop in the background
        let rpc_clone = rpc_server.clone();
        let server_task = tokio::spawn(async move {
            tracing::info!("Starting RPC server...");
            BaseChannel::with_defaults(server_chan)
                .execute(rpc_clone.serve())
                .for_each(|resp| async move {
                    tokio::spawn(resp);
                })
                .await;
            tracing::info!("RPC server stopped");
        });

        // Report cluster info immediately after connection
        if let Err(e) = rpc_server.report_cluster_info().await {
            tracing::error!("Failed to report cluster info: {}", e);
            // Don't fail the entire connection for this, just log and continue
        } else {
            tracing::info!("Successfully reported cluster info");
        }

        // Wait for the server task to complete
        if let Err(e) = server_task.await {
            return Err(anyhow!("RPC server task failed: {}", e));
        }

        tracing::debug!("Connection cycle completed, will retry in 1 second...");
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        Ok(())
    }

    async fn new_kube_client() -> Result<kube::Client> {
        let config = Self::retrieve_cluster_config().await?;
        let client = kube::Client::try_from(config)?;
        Ok(client)
    }

    async fn retrieve_cluster_config_from_home() -> Result<kube::Config> {
        Ok(kube::Config::infer().await?)
    }

    async fn retrieve_cluster_config() -> Result<kube::Config> {
        let retrive_from_home = std::env::var("KUBE_CLUSTER_FROM_HOME")
            .ok()
            .map(|v| v == "yes")
            .unwrap_or(false);
        if retrive_from_home {
            return Self::retrieve_cluster_config_from_home().await;
        }

        let key = yup_oauth2::read_service_account_key("/workspaces/key.json").await?;
        let project_id = key
            .project_id
            .clone()
            .ok_or_else(|| anyhow!("no project_id in key.json"))?;
        let authenticator = yup_oauth2::ServiceAccountAuthenticator::builder(key)
            .build()
            .await?;
        let token = authenticator.token(SCOPE).await?;
        let token = token.token().ok_or_else(|| anyhow!("no token"))?;
        let resp = reqwest::Client::new()
            .get(format!(
                "https://container.googleapis.com/v1/projects/{project_id}/locations/-/clusters"
            ))
            .header("Authorization", format!("Bearer {token}"))
            .send()
            .await?;
        let resp: ListClustersResponse = resp.json().await?;

        let cluster = resp
            .clusters
            .iter()
            .find(|c| c.name == "autopilot-belgium-production")
            .unwrap();
        let cert = STANDARD.decode(&cluster.master_auth.cluster_ca_certificate)?;
        let _cert = pem::parse_many(&cert)?
            .into_iter()
            .filter_map(|p| {
                if p.tag() == "CERTIFICATE" {
                    Some(p.into_contents())
                } else {
                    None
                }
            })
            .collect::<Vec<_>>();
        tracing::debug!("Cluster details: {:?}", cluster);
        let mut config = kube::Config::new(
            format!(
                "https://{}/",
                cluster
                    .control_plane_endpoints_config
                    .dns_endpoint_config
                    .endpoint
            )
            .parse()?,
        );
        // config.root_cert = Some(cert);
        config.auth_info = AuthInfo {
            token: Some(token.into()),
            ..Default::default()
        };
        // let client = kube::Client::try_from(config)?;
        // let api: kube::Api<Deployment> = kube::Api::all(client);
        // tracing::debug!("get api");
        // let r = api.list(&ListParams::default().limit(1)).await?;
        // let namespace = r.items.first().map(|d| d.namespace());
        // // client.
        // tracing::debug!("kube {namespace:?}");

        Ok(config)
    }
}

impl KubeManager {
    async fn collect_cluster_info(&self) -> Result<KubeClusterInfo> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

        // Get cluster version
        let version = client.apiserver_version().await?;

        // Get nodes and calculate cluster resources
        let nodes = self.get_cluster_nodes(client).await?;
        let (total_cpu_millicores, total_memory_bytes) = self.calculate_cluster_resources(&nodes);
        let node_count = nodes.len() as u32;
        let status = self.determine_cluster_status(&nodes, node_count);

        // Detect provider and region from node labels
        let (detected_provider, detected_region) = self.detect_provider_and_region(&nodes);

        // Get cluster identification
        let cluster_name = Self::get_cluster_name(client)
            .await
            .or_else(|| std::env::var("CLUSTER_NAME").ok());

        // Use detected values with environment variable fallbacks
        let provider = detected_provider.or_else(|| std::env::var("CLUSTER_PROVIDER").ok());
        let region = detected_region.or_else(|| std::env::var("CLUSTER_REGION").ok());

        Ok(KubeClusterInfo {
            cluster_name,
            cluster_version: format!("{}.{}", version.major, version.minor),
            node_count,
            available_cpu: format!("{}m", total_cpu_millicores),
            available_memory: format!("{}bytes", total_memory_bytes),
            provider,
            region,
            status,
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

    fn determine_cluster_status(
        &self,
        nodes: &[k8s_openapi::api::core::v1::Node],
        node_count: u32,
    ) -> KubeClusterStatus {
        let ready_nodes = nodes.iter().filter(|node| self.is_node_ready(node)).count();

        if ready_nodes == nodes.len() && node_count > 0 {
            KubeClusterStatus::Ready
        } else if ready_nodes > 0 {
            KubeClusterStatus::NotReady
        } else {
            KubeClusterStatus::Error
        }
    }

    fn is_node_ready(&self, node: &k8s_openapi::api::core::v1::Node) -> bool {
        node.status
            .as_ref()
            .and_then(|s| s.conditions.as_ref())
            .map(|conditions| {
                conditions
                    .iter()
                    .any(|c| c.type_ == "Ready" && c.status == "True")
            })
            .unwrap_or(false)
    }

    fn detect_provider_and_region(
        &self,
        nodes: &[k8s_openapi::api::core::v1::Node],
    ) -> (Option<String>, Option<String>) {
        let mut detected_provider = None;
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
                    if let Some(instance_type) = labels.get("node.kubernetes.io/instance-type") {
                        detected_provider = Self::detect_provider_from_instance_type(instance_type);
                    }
                }

                // Break early if we have both
                if detected_provider.is_some() && detected_region.is_some() {
                    break;
                }
            }
        }

        (detected_provider, detected_region)
    }

    async fn report_cluster_info(&self) -> Result<()> {
        let cluster_info = self.collect_cluster_info().await?;
        tracing::info!("Reporting cluster info: {:?}", cluster_info);

        match self
            .rpc_client
            .report_cluster_info(tarpc::context::current(), cluster_info)
            .await
        {
            Ok(_) => tracing::info!("Successfully reported cluster info"),
            Err(e) => tracing::error!("RPC call failed: {}", e),
        }

        Ok(())
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

    async fn get_cluster_name(client: &kube::Client) -> Option<String> {
        use k8s_openapi::api::core::v1::ConfigMap;

        let configmaps: kube::Api<ConfigMap> = kube::Api::namespaced(client.clone(), "kube-public");

        if let Ok(Some(cluster_info)) = configmaps.get_opt("cluster-info").await {
            return cluster_info.metadata.name.clone();
        }

        None
    }

    async fn collect_workloads(
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

    async fn collect_namespaces(&self) -> Result<Vec<KubeNamespace>> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;
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

            kube_namespaces.push(KubeNamespace {
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
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;
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

    async fn get_workload_by_name(
        &self,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

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

    fn clean_metadata(
        &self,
        original_metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
    ) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
        use k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta;

        ObjectMeta {
            name: original_metadata.name,
            labels: original_metadata.labels,
            ..Default::default()
        }
    }

    fn clean_service_spec(&self, service: Service) -> Service {
        use k8s_openapi::api::core::v1::ServiceSpec;

        // Create clean service spec with only essential fields
        let clean_spec = service.spec.map(|original_spec| ServiceSpec {
            // Only the essential fields for basic service functionality
            selector: original_spec.selector,
            ports: original_spec.ports,
            type_: original_spec.type_,

            // All other fields use defaults - let target cluster manage them
            ..Default::default()
        });

        // Create new clean service
        Service {
            metadata: self.clean_metadata(service.metadata),
            spec: clean_spec,
            status: None, // Never copy status
        }
    }

    fn clean_configmap(&self, configmap: ConfigMap) -> ConfigMap {
        ConfigMap {
            metadata: self.clean_metadata(configmap.metadata),
            data: configmap.data,
            binary_data: configmap.binary_data,
            immutable: configmap.immutable,
        }
    }

    fn clean_secret(&self, secret: Secret) -> Secret {
        Secret {
            metadata: self.clean_metadata(secret.metadata),
            data: secret.data,
            string_data: secret.string_data,
            type_: secret.type_,
            immutable: secret.immutable,
        }
    }

    fn merge_template_containers(
        &self,
        template: k8s_openapi::api::core::v1::PodTemplateSpec,
        workload_containers: &[KubeContainerInfo],
    ) -> k8s_openapi::api::core::v1::PodTemplateSpec {
        use k8s_openapi::api::core::v1::{PodSpec, PodTemplateSpec};
        use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

        let pod_spec = template.spec.map(|original_pod_spec| {
            let merged_containers = original_pod_spec
                .containers
                .into_iter()
                .map(|container| {
                    // Find matching container in workload by name
                    if let Some(workload_container) = workload_containers
                        .iter()
                        .find(|wc| wc.name == container.name)
                    {
                        let mut new_container = container.clone();

                        // Update image if specified in workload
                        if !workload_container.image.is_empty() {
                            new_container.image = Some(workload_container.image.clone());
                        }

                        // Update resource requirements
                        let mut resources = container.resources.unwrap_or_default();
                        let mut requests = resources.requests.unwrap_or_default();
                        let mut limits = resources.limits.unwrap_or_default();

                        // Update CPU and memory requests
                        if let Some(cpu_request) = &workload_container.cpu_request {
                            if !cpu_request.is_empty() {
                                requests.insert("cpu".to_string(), Quantity(cpu_request.clone()));
                            }
                        }
                        if let Some(memory_request) = &workload_container.memory_request {
                            if !memory_request.is_empty() {
                                requests
                                    .insert("memory".to_string(), Quantity(memory_request.clone()));
                            }
                        }

                        // Update CPU and memory limits
                        if let Some(cpu_limit) = &workload_container.cpu_limit {
                            if !cpu_limit.is_empty() {
                                limits.insert("cpu".to_string(), Quantity(cpu_limit.clone()));
                            }
                        }
                        if let Some(memory_limit) = &workload_container.memory_limit {
                            if !memory_limit.is_empty() {
                                limits.insert("memory".to_string(), Quantity(memory_limit.clone()));
                            }
                        }

                        // Set the updated resources back
                        resources.requests = if requests.is_empty() {
                            None
                        } else {
                            Some(requests)
                        };
                        resources.limits = if limits.is_empty() {
                            None
                        } else {
                            Some(limits)
                        };
                        new_container.resources = Some(resources);

                        new_container
                    } else {
                        container
                    }
                })
                .collect();

            PodSpec {
                containers: merged_containers,
                ..original_pod_spec
            }
        });

        PodTemplateSpec {
            spec: pod_spec,
            ..template
        }
    }

    fn clean_deployment(
        &self,
        deployment: Deployment,
        workload_containers: &[KubeContainerInfo],
    ) -> Deployment {
        use k8s_openapi::api::apps::v1::DeploymentSpec;

        let clean_spec = deployment.spec.map(|original_spec| {
            // Merge container specs with template
            let template =
                self.merge_template_containers(original_spec.template, workload_containers);

            DeploymentSpec {
                // Only the essential fields for basic deployment functionality
                replicas: original_spec.replicas,
                selector: original_spec.selector,
                template,
                min_ready_seconds: original_spec.min_ready_seconds,
                paused: original_spec.paused,
                progress_deadline_seconds: original_spec.progress_deadline_seconds,
                revision_history_limit: original_spec.revision_history_limit,
                strategy: original_spec.strategy,
            }
        });

        Deployment {
            metadata: self.clean_metadata(deployment.metadata),
            spec: clean_spec,
            status: None, // Never copy status
        }
    }

    fn clean_statefulset(
        &self,
        statefulset: StatefulSet,
        workload_containers: &[KubeContainerInfo],
    ) -> StatefulSet {
        use k8s_openapi::api::apps::v1::StatefulSetSpec;

        let clean_spec = statefulset.spec.map(|original_spec| {
            // Merge container specs with template
            let template =
                self.merge_template_containers(original_spec.template, workload_containers);

            StatefulSetSpec {
                service_name: original_spec.service_name,
                replicas: original_spec.replicas,
                selector: original_spec.selector,
                template,
                volume_claim_templates: original_spec.volume_claim_templates,
                update_strategy: original_spec.update_strategy,
                min_ready_seconds: original_spec.min_ready_seconds,
                persistent_volume_claim_retention_policy: original_spec
                    .persistent_volume_claim_retention_policy,
                ordinals: original_spec.ordinals,
                revision_history_limit: original_spec.revision_history_limit,
                pod_management_policy: original_spec.pod_management_policy,
            }
        });

        StatefulSet {
            metadata: self.clean_metadata(statefulset.metadata),
            spec: clean_spec,
            status: None,
        }
    }

    fn clean_daemonset(
        &self,
        daemonset: DaemonSet,
        workload_containers: &[KubeContainerInfo],
    ) -> DaemonSet {
        use k8s_openapi::api::apps::v1::DaemonSetSpec;

        let clean_spec = daemonset.spec.map(|original_spec| {
            // Merge container specs with template
            let template =
                self.merge_template_containers(original_spec.template, workload_containers);

            DaemonSetSpec {
                selector: original_spec.selector,
                template,
                update_strategy: original_spec.update_strategy,
                min_ready_seconds: original_spec.min_ready_seconds,
                revision_history_limit: original_spec.revision_history_limit,
            }
        });

        DaemonSet {
            metadata: self.clean_metadata(daemonset.metadata),
            spec: clean_spec,
            status: None,
        }
    }

    fn merge_containers(
        &self,
        containers: Vec<k8s_openapi::api::core::v1::Container>,
        workload_containers: &[KubeContainerInfo],
    ) -> Vec<k8s_openapi::api::core::v1::Container> {
        use k8s_openapi::apimachinery::pkg::api::resource::Quantity;

        containers
            .into_iter()
            .map(|container| {
                // Find matching container in workload by name
                if let Some(workload_container) = workload_containers
                    .iter()
                    .find(|wc| wc.name == container.name)
                {
                    let mut new_container = container.clone();

                    // Update image if specified in workload
                    if !workload_container.image.is_empty() {
                        new_container.image = Some(workload_container.image.clone());
                    }

                    // Update resource requirements
                    let mut resources = container.resources.unwrap_or_default();
                    let mut requests = resources.requests.unwrap_or_default();
                    let mut limits = resources.limits.unwrap_or_default();

                    // Update CPU and memory requests
                    if let Some(cpu_request) = &workload_container.cpu_request {
                        if !cpu_request.is_empty() {
                            requests.insert("cpu".to_string(), Quantity(cpu_request.clone()));
                        }
                    }
                    if let Some(memory_request) = &workload_container.memory_request {
                        if !memory_request.is_empty() {
                            requests.insert("memory".to_string(), Quantity(memory_request.clone()));
                        }
                    }

                    // Update CPU and memory limits
                    if let Some(cpu_limit) = &workload_container.cpu_limit {
                        if !cpu_limit.is_empty() {
                            limits.insert("cpu".to_string(), Quantity(cpu_limit.clone()));
                        }
                    }
                    if let Some(memory_limit) = &workload_container.memory_limit {
                        if !memory_limit.is_empty() {
                            limits.insert("memory".to_string(), Quantity(memory_limit.clone()));
                        }
                    }

                    // Set the updated resources back
                    resources.requests = if requests.is_empty() {
                        None
                    } else {
                        Some(requests)
                    };
                    resources.limits = if limits.is_empty() {
                        None
                    } else {
                        Some(limits)
                    };
                    new_container.resources = Some(resources);

                    new_container
                } else {
                    container
                }
            })
            .collect()
    }

    fn clean_pod(&self, pod: Pod, workload_containers: &[KubeContainerInfo]) -> Pod {
        use k8s_openapi::api::core::v1::PodSpec;

        let clean_spec = pod.spec.map(|original_spec| {
            let merged_containers =
                self.merge_containers(original_spec.containers, workload_containers);

            PodSpec {
                active_deadline_seconds: original_spec.active_deadline_seconds,
                containers: merged_containers,
                init_containers: original_spec.init_containers,
                ephemeral_containers: original_spec.ephemeral_containers,
                volumes: original_spec.volumes,
                restart_policy: original_spec.restart_policy,
                termination_grace_period_seconds: original_spec.termination_grace_period_seconds,
                dns_policy: original_spec.dns_policy,
                dns_config: original_spec.dns_config,
                node_selector: original_spec.node_selector,
                service_account_name: original_spec.service_account_name,
                service_account: original_spec.service_account,
                automount_service_account_token: original_spec.automount_service_account_token,
                security_context: original_spec.security_context,
                image_pull_secrets: original_spec.image_pull_secrets,
                affinity: original_spec.affinity,
                tolerations: original_spec.tolerations,
                topology_spread_constraints: original_spec.topology_spread_constraints,
                priority_class_name: original_spec.priority_class_name,
                priority: original_spec.priority,
                preemption_policy: original_spec.preemption_policy,
                overhead: original_spec.overhead,
                enable_service_links: original_spec.enable_service_links,
                os: original_spec.os,
                host_users: original_spec.host_users,
                scheduling_gates: original_spec.scheduling_gates,
                resource_claims: original_spec.resource_claims,

                // Remove runtime/node-specific fields by using Default
                ..Default::default()
            }
        });

        Pod {
            metadata: self.clean_metadata(pod.metadata),
            spec: clean_spec,
            status: None,
        }
    }

    fn clean_job(&self, job: Job, workload_containers: &[KubeContainerInfo]) -> Job {
        use k8s_openapi::api::batch::v1::JobSpec;

        let clean_spec = job.spec.map(|original_spec| {
            // Merge container specs with template
            let template =
                self.merge_template_containers(original_spec.template, workload_containers);

            JobSpec {
                template,
                parallelism: original_spec.parallelism,
                completions: original_spec.completions,
                completion_mode: original_spec.completion_mode,
                active_deadline_seconds: original_spec.active_deadline_seconds,
                backoff_limit: original_spec.backoff_limit,
                backoff_limit_per_index: original_spec.backoff_limit_per_index,
                max_failed_indexes: original_spec.max_failed_indexes,
                selector: original_spec.selector,
                manual_selector: original_spec.manual_selector,
                ttl_seconds_after_finished: original_spec.ttl_seconds_after_finished,
                suspend: original_spec.suspend,
                pod_failure_policy: original_spec.pod_failure_policy,
                pod_replacement_policy: original_spec.pod_replacement_policy,
                managed_by: original_spec.managed_by,
                success_policy: original_spec.success_policy,
            }
        });

        Job {
            metadata: self.clean_metadata(job.metadata),
            spec: clean_spec,
            status: None,
        }
    }

    fn clean_cronjob(
        &self,
        cronjob: CronJob,
        workload_containers: &[KubeContainerInfo],
    ) -> CronJob {
        use k8s_openapi::api::batch::v1::CronJobSpec;

        let clean_spec = cronjob.spec.map(|original_spec| {
            // Handle job_template which contains a JobTemplateSpec
            let job_template = {
                let mut template = original_spec.job_template;
                if let Some(job_spec) = &mut template.spec {
                    // Merge container specs with the job template's template
                    job_spec.template = self
                        .merge_template_containers(job_spec.template.clone(), workload_containers);
                }
                template
            };

            CronJobSpec {
                schedule: original_spec.schedule,
                time_zone: original_spec.time_zone,
                starting_deadline_seconds: original_spec.starting_deadline_seconds,
                concurrency_policy: original_spec.concurrency_policy,
                suspend: original_spec.suspend,
                job_template,
                successful_jobs_history_limit: original_spec.successful_jobs_history_limit,
                failed_jobs_history_limit: original_spec.failed_jobs_history_limit,
            }
        });

        CronJob {
            metadata: self.clean_metadata(cronjob.metadata),
            spec: clean_spec,
            status: None,
        }
    }

    fn clean_replicaset(
        &self,
        replicaset: ReplicaSet,
        workload_containers: &[KubeContainerInfo],
    ) -> ReplicaSet {
        use k8s_openapi::api::apps::v1::ReplicaSetSpec;

        let clean_spec = replicaset.spec.map(|original_spec| {
            // Merge container specs with template
            let template = original_spec
                .template
                .map(|t| self.merge_template_containers(t, workload_containers));

            ReplicaSetSpec {
                replicas: original_spec.replicas,
                selector: original_spec.selector,
                template,
                min_ready_seconds: original_spec.min_ready_seconds,
            }
        });

        ReplicaSet {
            metadata: self.clean_metadata(replicaset.metadata),
            spec: clean_spec,
            status: None,
        }
    }

    async fn retrieve_cluster_ip_services_in_namespace(
        &self,
        client: &kube::Client,
        namespace: &str,
    ) -> Result<Vec<Service>> {
        let services_api: kube::Api<Service> = kube::Api::namespaced((*client).clone(), namespace);

        let mut all_services = Vec::new();

        // Retrieve all services in the namespace
        let mut continue_token: Option<String> = None;
        loop {
            let mut list_params = ListParams::default().limit(100);
            if let Some(token) = &continue_token {
                list_params = list_params.continue_token(token);
            }

            let services_list = services_api.list(&list_params).await?;

            // Filter for ClusterIP services only (other types are cluster-specific)
            let cluster_ip_services: Vec<Service> = services_list
                .items
                .into_iter()
                .filter(|service| {
                    service
                        .spec
                        .as_ref()
                        .map(|spec| {
                            spec.type_.as_deref() == Some("ClusterIP") || spec.type_.is_none()
                        })
                        .unwrap_or(false)
                })
                .collect();

            all_services.extend(cluster_ip_services);

            continue_token = services_list.metadata.continue_;
            if continue_token.is_none() {
                break;
            }
        }

        Ok(all_services)
    }

    async fn retrieve_workloads_yaml(
        &self,
        catalog_workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<KubeWorkloadsWithResources> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

        let mut workloads = Vec::new();
        let mut all_services_set_by_namespace: HashMap<String, std::collections::HashSet<String>> =
            HashMap::new();
        let mut all_configmaps_set_by_namespace: HashMap<
            String,
            std::collections::HashSet<String>,
        > = HashMap::new();
        let mut all_secrets_set_by_namespace: HashMap<String, std::collections::HashSet<String>> =
            HashMap::new();

        // Store all resources by namespace for later serialization
        let mut all_services_by_namespace: HashMap<String, Vec<Service>> = HashMap::new();

        // Group workloads by namespace
        let mut workloads_by_namespace: std::collections::HashMap<
            String,
            Vec<KubeAppCatalogWorkload>,
        > = std::collections::HashMap::new();
        for catalog_workload in catalog_workloads {
            workloads_by_namespace
                .entry(catalog_workload.namespace.clone())
                .or_insert_with(Vec::new)
                .push(catalog_workload);
        }

        // Process each namespace separately
        for (namespace, namespace_workloads) in workloads_by_namespace {
            // Get all ClusterIP services once for this namespace and reuse for all workloads
            let all_services = self
                .retrieve_cluster_ip_services_in_namespace(client, &namespace)
                .await?;

            // Store services by namespace for later serialization
            all_services_by_namespace.insert(namespace.clone(), all_services.clone());

            // Process each workload in this namespace
            for workload in namespace_workloads {
                let workload_yaml_result = self
                    .retrieve_single_workload_with_resources(client, &workload, &all_services)
                    .await?;

                // Helper to collect resource names by namespace
                let mut collect_resources =
                    |services: Vec<String>, configmaps: Vec<String>, secrets: Vec<String>| {
                        let services_set = all_services_set_by_namespace
                            .entry(namespace.clone())
                            .or_default();
                        let configmaps_set = all_configmaps_set_by_namespace
                            .entry(namespace.clone())
                            .or_default();
                        let secrets_set = all_secrets_set_by_namespace
                            .entry(namespace.clone())
                            .or_default();

                        for service in services {
                            services_set.insert(service);
                        }
                        for configmap in configmaps {
                            configmaps_set.insert(configmap);
                        }
                        for secret in secrets {
                            secrets_set.insert(secret);
                        }
                    };

                // Extract just the workload YAML and collect associated resources
                match workload_yaml_result {
                    KubeWorkloadYaml::Deployment(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::Deployment(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                    KubeWorkloadYaml::StatefulSet(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::StatefulSet(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                    KubeWorkloadYaml::DaemonSet(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::DaemonSet(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                    KubeWorkloadYaml::ReplicaSet(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::ReplicaSet(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                    KubeWorkloadYaml::Pod(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::Pod(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                    KubeWorkloadYaml::Job(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::Job(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                    KubeWorkloadYaml::CronJob(ws) => {
                        workloads.push(KubeWorkloadYamlOnly::CronJob(ws.workload_yaml));
                        collect_resources(ws.services, ws.configmaps, ws.secrets);
                    }
                }
            }

            tracing::debug!(
                "Retrieved workloads with services from namespace {}",
                namespace
            );
        }

        // Now build the actual YAML content maps from the already-fetched resources
        let (services_yaml_map, configmaps_yaml_map, secrets_yaml_map) = self
            .build_resource_yaml_maps(
                client,
                all_services_by_namespace,
                &all_services_set_by_namespace,
                &all_configmaps_set_by_namespace,
                &all_secrets_set_by_namespace,
            )
            .await?;

        Ok(KubeWorkloadsWithResources {
            workloads,
            services: services_yaml_map,
            configmaps: configmaps_yaml_map,
            secrets: secrets_yaml_map,
        })
    }

    async fn retrieve_single_workload_with_resources(
        &self,
        client: &kube::Client,
        workload: &KubeAppCatalogWorkload,
        all_services: &[Service],
    ) -> Result<KubeWorkloadYaml> {
        match workload.kind {
            KubeWorkloadKind::Deployment => {
                let api: kube::Api<Deployment> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let deployment = api.get(&workload.name).await?;

                // Get labels for service matching
                let workload_labels = deployment
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.metadata.as_ref())
                    .and_then(|m| m.labels.as_ref())
                    .cloned()
                    .unwrap_or_default();

                // Find matching services from pre-loaded services
                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let deployment_json = serde_json::to_value(&deployment)?;
                let configmaps = Self::extract_configmap_references(&deployment_json);
                let secrets = Self::extract_secret_references(&deployment_json);

                // Clean server-managed fields and merge container specs
                let clean_deployment = self.clean_deployment(deployment, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_deployment)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::Deployment(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
            KubeWorkloadKind::StatefulSet => {
                let api: kube::Api<StatefulSet> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let statefulset = api.get(&workload.name).await?;

                let workload_labels = statefulset
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.metadata.as_ref())
                    .and_then(|m| m.labels.as_ref())
                    .cloned()
                    .unwrap_or_default();

                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let statefulset_json = serde_json::to_value(&statefulset)?;
                let configmaps = Self::extract_configmap_references(&statefulset_json);
                let secrets = Self::extract_secret_references(&statefulset_json);

                let clean_statefulset = self.clean_statefulset(statefulset, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_statefulset)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::StatefulSet(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
            KubeWorkloadKind::DaemonSet => {
                let api: kube::Api<DaemonSet> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let daemonset = api.get(&workload.name).await?;

                let workload_labels = daemonset
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.metadata.as_ref())
                    .and_then(|m| m.labels.as_ref())
                    .cloned()
                    .unwrap_or_default();

                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let daemonset_json = serde_json::to_value(&daemonset)?;
                let configmaps = Self::extract_configmap_references(&daemonset_json);
                let secrets = Self::extract_secret_references(&daemonset_json);

                let clean_daemonset = self.clean_daemonset(daemonset, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_daemonset)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::DaemonSet(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
            KubeWorkloadKind::Pod => {
                let api: kube::Api<Pod> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let pod = api.get(&workload.name).await?;

                let workload_labels = pod.metadata.labels.as_ref().cloned().unwrap_or_default();
                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let pod_json = serde_json::to_value(&pod)?;
                let configmaps = Self::extract_configmap_references(&pod_json);
                let secrets = Self::extract_secret_references(&pod_json);

                let clean_pod = self.clean_pod(pod, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_pod)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::Pod(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
            KubeWorkloadKind::Job => {
                let api: kube::Api<Job> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let job = api.get(&workload.name).await?;

                let workload_labels = job
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.metadata.as_ref())
                    .and_then(|m| m.labels.as_ref())
                    .cloned()
                    .unwrap_or_default();

                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let job_json = serde_json::to_value(&job)?;
                let configmaps = Self::extract_configmap_references(&job_json);
                let secrets = Self::extract_secret_references(&job_json);

                let clean_job = self.clean_job(job, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_job)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::Job(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
            KubeWorkloadKind::CronJob => {
                let api: kube::Api<CronJob> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let cronjob = api.get(&workload.name).await?;

                let workload_labels = cronjob
                    .spec
                    .as_ref()
                    .and_then(|s| s.job_template.spec.as_ref())
                    .and_then(|js| js.template.metadata.as_ref())
                    .and_then(|m| m.labels.as_ref())
                    .cloned()
                    .unwrap_or_default();

                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let cronjob_json = serde_json::to_value(&cronjob)?;
                let configmaps = Self::extract_configmap_references(&cronjob_json);
                let secrets = Self::extract_secret_references(&cronjob_json);

                let clean_cronjob = self.clean_cronjob(cronjob, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_cronjob)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::CronJob(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
            KubeWorkloadKind::ReplicaSet => {
                let api: kube::Api<ReplicaSet> =
                    kube::Api::namespaced((*client).clone(), &workload.namespace);
                let replicaset = api.get(&workload.name).await?;

                let workload_labels = replicaset
                    .spec
                    .as_ref()
                    .and_then(|s| s.template.as_ref())
                    .and_then(|t| t.metadata.as_ref())
                    .and_then(|m| m.labels.as_ref())
                    .cloned()
                    .unwrap_or_default();

                let services =
                    self.find_matching_services_from_list(&workload_labels, all_services)?;

                // Extract ConfigMap and Secret references
                let replicaset_json = serde_json::to_value(&replicaset)?;
                let configmaps = Self::extract_configmap_references(&replicaset_json);
                let secrets = Self::extract_secret_references(&replicaset_json);

                let clean_replicaset = self.clean_replicaset(replicaset, &workload.containers);
                let workload_yaml = serde_yaml::to_string(&clean_replicaset)
                    .map_err(|e| anyhow!("Failed to serialize to YAML: {}", e))?;

                Ok(KubeWorkloadYaml::ReplicaSet(KubeWorkloadWithServices {
                    workload_yaml,
                    services,
                    configmaps,
                    secrets,
                }))
            }
        }
    }

    fn find_matching_services_from_list(
        &self,
        workload_labels: &std::collections::BTreeMap<String, String>,
        all_services: &[Service],
    ) -> Result<Vec<String>> {
        let mut matching_service_names = Vec::new();

        for service in all_services {
            if let Some(selector) = &service.spec.as_ref().and_then(|s| s.selector.as_ref()) {
                let matches = selector
                    .iter()
                    .all(|(key, value)| workload_labels.get(key).map_or(false, |v| v == value));

                if matches && !selector.is_empty() {
                    if let Some(service_name) = &service.metadata.name {
                        matching_service_names.push(service_name.clone());
                    }
                }
            }
        }

        Ok(matching_service_names)
    }

    fn extract_configmap_references(workload_spec: &serde_json::Value) -> Vec<String> {
        let mut configmap_names = std::collections::HashSet::new();

        // Extract ConfigMap references from various places in the spec
        Self::extract_from_containers(workload_spec, &mut configmap_names, "configMap");
        Self::extract_from_volumes(workload_spec, &mut configmap_names, "configMap");

        configmap_names.into_iter().collect()
    }

    fn extract_secret_references(workload_spec: &serde_json::Value) -> Vec<String> {
        let mut secret_names = std::collections::HashSet::new();

        // Extract Secret references from various places in the spec
        Self::extract_from_containers(workload_spec, &mut secret_names, "secret");
        Self::extract_from_volumes(workload_spec, &mut secret_names, "secret");
        Self::extract_image_pull_secrets(workload_spec, &mut secret_names);

        secret_names.into_iter().collect()
    }

    fn extract_from_containers(
        spec: &serde_json::Value,
        names: &mut std::collections::HashSet<String>,
        resource_type: &str,
    ) {
        // Look in spec.template.spec.containers (for Deployments, StatefulSets, etc.)
        if let Some(containers) = spec
            .pointer("/spec/template/spec/containers")
            .and_then(|c| c.as_array())
        {
            for container in containers {
                Self::extract_from_container_env(container, names, resource_type);
                Self::extract_from_container_env_from(container, names, resource_type);
                Self::extract_from_container_volume_mounts(container, spec, names, resource_type);
            }
        }

        // Look in spec.template.spec.initContainers (for Deployments, StatefulSets, etc.)
        if let Some(init_containers) = spec
            .pointer("/spec/template/spec/initContainers")
            .and_then(|c| c.as_array())
        {
            for container in init_containers {
                Self::extract_from_container_env(container, names, resource_type);
                Self::extract_from_container_env_from(container, names, resource_type);
                Self::extract_from_container_volume_mounts(container, spec, names, resource_type);
            }
        }

        // Look in spec.containers (for Pods)
        if let Some(containers) = spec.pointer("/spec/containers").and_then(|c| c.as_array()) {
            for container in containers {
                Self::extract_from_container_env(container, names, resource_type);
                Self::extract_from_container_env_from(container, names, resource_type);
                Self::extract_from_container_volume_mounts(container, spec, names, resource_type);
            }
        }

        // Look in spec.initContainers (for Pods)
        if let Some(init_containers) = spec
            .pointer("/spec/initContainers")
            .and_then(|c| c.as_array())
        {
            for container in init_containers {
                Self::extract_from_container_env(container, names, resource_type);
                Self::extract_from_container_env_from(container, names, resource_type);
                Self::extract_from_container_volume_mounts(container, spec, names, resource_type);
            }
        }
    }

    fn extract_from_container_env(
        container: &serde_json::Value,
        names: &mut std::collections::HashSet<String>,
        resource_type: &str,
    ) {
        if let Some(env_vars) = container.pointer("/env").and_then(|e| e.as_array()) {
            for env_var in env_vars {
                if let Some(value_from) = env_var.get("valueFrom") {
                    let key_ref_name = if resource_type == "configMap" {
                        "configMapKeyRef"
                    } else {
                        "secretKeyRef"
                    };
                    if let Some(name) = value_from
                        .pointer(&format!("/{}/name", key_ref_name))
                        .and_then(|n| n.as_str())
                    {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }

    fn extract_from_container_env_from(
        container: &serde_json::Value,
        names: &mut std::collections::HashSet<String>,
        resource_type: &str,
    ) {
        if let Some(env_from) = container.pointer("/envFrom").and_then(|e| e.as_array()) {
            for env_from_source in env_from {
                let ref_name = if resource_type == "configMap" {
                    "configMapRef"
                } else {
                    "secretRef"
                };
                if let Some(name) = env_from_source
                    .pointer(&format!("/{}/name", ref_name))
                    .and_then(|n| n.as_str())
                {
                    names.insert(name.to_string());
                }
            }
        }
    }

    fn extract_from_container_volume_mounts(
        container: &serde_json::Value,
        spec: &serde_json::Value,
        names: &mut std::collections::HashSet<String>,
        resource_type: &str,
    ) {
        if let Some(volume_mounts) = container
            .pointer("/volumeMounts")
            .and_then(|v| v.as_array())
        {
            for volume_mount in volume_mounts {
                if let Some(volume_name) = volume_mount.get("name").and_then(|n| n.as_str()) {
                    // Find the corresponding volume in spec
                    Self::find_volume_source(spec, volume_name, names, resource_type);
                }
            }
        }
    }

    fn extract_from_volumes(
        spec: &serde_json::Value,
        names: &mut std::collections::HashSet<String>,
        resource_type: &str,
    ) {
        // Look in spec.template.spec.volumes (for Deployments, StatefulSets, etc.)
        if let Some(volumes) = spec
            .pointer("/spec/template/spec/volumes")
            .and_then(|v| v.as_array())
        {
            for volume in volumes {
                let name_field = if resource_type == "secret" {
                    "secretName"
                } else {
                    "name"
                };
                if let Some(name) = volume
                    .pointer(&format!("/{}/{}", resource_type, name_field))
                    .and_then(|n| n.as_str())
                {
                    names.insert(name.to_string());
                }
            }
        }

        // Look in spec.volumes (for Pods)
        if let Some(volumes) = spec.pointer("/spec/volumes").and_then(|v| v.as_array()) {
            for volume in volumes {
                let name_field = if resource_type == "secret" {
                    "secretName"
                } else {
                    "name"
                };
                if let Some(name) = volume
                    .pointer(&format!("/{}/{}", resource_type, name_field))
                    .and_then(|n| n.as_str())
                {
                    names.insert(name.to_string());
                }
            }
        }
    }

    fn find_volume_source(
        spec: &serde_json::Value,
        volume_name: &str,
        names: &mut std::collections::HashSet<String>,
        resource_type: &str,
    ) {
        let volume_paths = [
            "/spec/template/spec/volumes", // For Deployments, StatefulSets, etc.
            "/spec/volumes",               // For Pods
        ];

        for volume_path in &volume_paths {
            if let Some(volumes) = spec.pointer(volume_path).and_then(|v| v.as_array()) {
                for volume in volumes {
                    if let Some(name) = volume.get("name").and_then(|n| n.as_str()) {
                        if name == volume_name {
                            if let Some(resource_name) = volume
                                .pointer(&format!("/{}/name", resource_type))
                                .and_then(|n| n.as_str())
                            {
                                names.insert(resource_name.to_string());
                            }
                            return;
                        }
                    }
                }
            }
        }
    }

    fn extract_image_pull_secrets(
        spec: &serde_json::Value,
        names: &mut std::collections::HashSet<String>,
    ) {
        let pull_secret_paths = [
            "/spec/template/spec/imagePullSecrets", // For Deployments, StatefulSets, etc.
            "/spec/imagePullSecrets",               // For Pods
        ];

        for path in &pull_secret_paths {
            if let Some(pull_secrets) = spec.pointer(path).and_then(|p| p.as_array()) {
                for pull_secret in pull_secrets {
                    if let Some(name) = pull_secret.get("name").and_then(|n| n.as_str()) {
                        names.insert(name.to_string());
                    }
                }
            }
        }
    }

    async fn build_resource_yaml_maps(
        &self,
        client: &kube::Client,
        all_services_by_namespace: HashMap<String, Vec<Service>>,
        needed_services_by_namespace: &HashMap<String, std::collections::HashSet<String>>,
        needed_configmaps_by_namespace: &HashMap<String, std::collections::HashSet<String>>,
        needed_secrets_by_namespace: &HashMap<String, std::collections::HashSet<String>>,
    ) -> Result<(
        HashMap<String, String>,
        HashMap<String, String>,
        HashMap<String, String>,
    )> {
        let mut services_yaml_map = HashMap::new();
        let mut configmaps_yaml_map = HashMap::new();
        let mut secrets_yaml_map = HashMap::new();

        // Serialize only the services that are actually needed
        for (namespace, services) in all_services_by_namespace {
            if let Some(needed_services) = needed_services_by_namespace.get(&namespace) {
                for service in services {
                    if let Some(service_name) = &service.metadata.name {
                        if needed_services.contains(service_name) {
                            let clean_service = self.clean_service_spec(service.clone());

                            if let Ok(service_yaml) = serde_yaml::to_string(&clean_service) {
                                services_yaml_map.insert(service_name.clone(), service_yaml);
                            }
                        }
                    }
                }
            }
        }

        // Fetch and serialize only the configmaps that are actually needed
        for (namespace, needed_configmaps) in needed_configmaps_by_namespace {
            let configmaps_api: kube::Api<ConfigMap> =
                kube::Api::namespaced((*client).clone(), namespace);

            for configmap_name in needed_configmaps {
                match configmaps_api.get(configmap_name).await {
                    Ok(configmap) => {
                        let clean_configmap = self.clean_configmap(configmap);

                        if let Ok(configmap_yaml) = serde_yaml::to_string(&clean_configmap) {
                            configmaps_yaml_map.insert(configmap_name.clone(), configmap_yaml);
                        }
                    }
                    Err(e) => {
                        // Log error but continue - ConfigMap might not exist or be accessible
                        tracing::warn!(
                            "Could not fetch ConfigMap {}/{}: {}",
                            namespace,
                            configmap_name,
                            e
                        );
                    }
                }
            }
        }

        // Fetch and serialize only the secrets that are actually needed
        for (namespace, needed_secrets) in needed_secrets_by_namespace {
            let secrets_api: kube::Api<Secret> =
                kube::Api::namespaced((*client).clone(), namespace);

            for secret_name in needed_secrets {
                match secrets_api.get(secret_name).await {
                    Ok(secret) => {
                        let clean_secret = self.clean_secret(secret);

                        if let Ok(secret_yaml) = serde_yaml::to_string(&clean_secret) {
                            secrets_yaml_map.insert(secret_name.clone(), secret_yaml);
                        }
                    }
                    Err(e) => {
                        // Log error but continue - Secret might not exist or be accessible
                        tracing::warn!(
                            "Could not fetch Secret {}/{}: {}",
                            namespace,
                            secret_name,
                            e
                        );
                    }
                }
            }
        }

        Ok((services_yaml_map, configmaps_yaml_map, secrets_yaml_map))
    }

    async fn apply_workload_yaml(
        &self,
        namespace: String,
        workload_yaml: KubeWorkloadYaml,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

        // Step 1: Ensure namespace exists
        self.ensure_namespace_exists(&namespace).await?;

        // Step 2: Apply the resource based on workload type
        match workload_yaml {
            KubeWorkloadYaml::Deployment(workload_with_services) => {
                tracing::info!(
                    "Applying Deployment to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_deployment(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
            KubeWorkloadYaml::StatefulSet(workload_with_services) => {
                tracing::info!(
                    "Applying StatefulSet to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_statefulset(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
            KubeWorkloadYaml::DaemonSet(workload_with_services) => {
                tracing::info!(
                    "Applying DaemonSet to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_daemonset(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
            KubeWorkloadYaml::Pod(workload_with_services) => {
                tracing::info!(
                    "Applying Pod to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_pod(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
            KubeWorkloadYaml::Job(workload_with_services) => {
                tracing::info!(
                    "Applying Job to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_job(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
            KubeWorkloadYaml::CronJob(workload_with_services) => {
                tracing::info!(
                    "Applying CronJob to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_cronjob(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
            KubeWorkloadYaml::ReplicaSet(workload_with_services) => {
                tracing::info!(
                    "Applying ReplicaSet to namespace '{}' with labels: {:?}",
                    namespace,
                    labels
                );
                self.apply_replicaset(
                    client,
                    &namespace,
                    &workload_with_services.workload_yaml,
                    &labels,
                )
                .await?;
                self.apply_services(
                    client,
                    &namespace,
                    &workload_with_services.services,
                    &labels,
                )
                .await?;
                self.apply_configmaps(
                    client,
                    &namespace,
                    &workload_with_services.configmaps,
                    &labels,
                )
                .await?;
                self.apply_secrets(client, &namespace, &workload_with_services.secrets, &labels)
                    .await?;
            }
        }

        tracing::info!("Successfully applied workload to namespace '{}'", namespace);
        Ok(())
    }

    async fn apply_workloads_with_resources(
        &self,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

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
        for (name, service_yaml) in &workloads_with_resources.services {
            tracing::info!("Applying Service '{}' to namespace '{}'", name, namespace);
            self.apply_single_service(client, &namespace, service_yaml, &labels)
                .await?;
        }

        tracing::info!(
            "Successfully applied all workloads and resources to namespace '{}'",
            namespace
        );
        Ok(())
    }

    async fn apply_services(
        &self,
        client: &kube::Client,
        namespace: &str,
        service_yamls: &[String],
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        for service_yaml in service_yamls {
            let mut service: Service = serde_yaml::from_str(service_yaml)?;

            // Add environment labels to service
            self.add_labels_to_metadata(&mut service.metadata, labels);

            // Force namespace
            service.metadata.namespace = Some(namespace.to_string());

            let services_api: kube::Api<Service> =
                kube::Api::namespaced((*client).clone(), namespace);

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
        }
        Ok(())
    }

    async fn apply_configmaps(
        &self,
        client: &kube::Client,
        namespace: &str,
        configmap_yamls: &[String],
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        for configmap_yaml in configmap_yamls {
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
        }
        Ok(())
    }

    async fn apply_secrets(
        &self,
        client: &kube::Client,
        namespace: &str,
        secret_yamls: &[String],
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        for secret_yaml in secret_yamls {
            let mut secret: Secret = serde_yaml::from_str(secret_yaml)?;

            // Add environment labels to secret
            self.add_labels_to_metadata(&mut secret.metadata, labels);

            // Force namespace
            secret.metadata.namespace = Some(namespace.to_string());

            let secrets_api: kube::Api<Secret> =
                kube::Api::namespaced((*client).clone(), namespace);

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
        }
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
                self.apply_deployment(client, namespace, yaml, labels)
                    .await?;
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
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

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

    async fn apply_deployment(
        &self,
        client: &kube::Client,
        namespace: &str,
        yaml_manifest: &str,
        labels: &std::collections::HashMap<String, String>,
    ) -> Result<()> {
        let mut deployment: Deployment = serde_yaml::from_str(yaml_manifest)?;

        // Add environment labels to deployment
        self.add_labels_to_metadata(&mut deployment.metadata, labels);

        // Force namespace
        deployment.metadata.namespace = Some(namespace.to_string());

        let api: kube::Api<Deployment> = kube::Api::namespaced((*client).clone(), namespace);

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

    async fn get_workload_resource_details(
        &self,
        name: &str,
        namespace: &str,
        kind: &KubeWorkloadKind,
    ) -> Result<Vec<KubeContainerInfo>> {
        let client = self
            .kube_client
            .as_ref()
            .ok_or_else(|| anyhow!("Kubernetes client not available"))?;

        match kind {
            KubeWorkloadKind::Deployment => {
                let api: kube::Api<Deployment> =
                    kube::Api::namespaced((**client).clone(), namespace);
                if let Ok(deployment) = api.get(name).await {
                    if let Some(spec) = &deployment.spec {
                        if let Some(pod_spec) = &spec.template.spec {
                            return self.extract_pod_resource_info(pod_spec);
                        }
                    }
                }
            }
            KubeWorkloadKind::StatefulSet => {
                let api: kube::Api<StatefulSet> =
                    kube::Api::namespaced((**client).clone(), namespace);
                if let Ok(statefulset) = api.get(name).await {
                    if let Some(spec) = &statefulset.spec {
                        if let Some(pod_spec) = &spec.template.spec {
                            return self.extract_pod_resource_info(pod_spec);
                        }
                    }
                }
            }
            KubeWorkloadKind::DaemonSet => {
                let api: kube::Api<DaemonSet> =
                    kube::Api::namespaced((**client).clone(), namespace);
                if let Ok(daemonset) = api.get(name).await {
                    if let Some(spec) = &daemonset.spec {
                        if let Some(pod_spec) = &spec.template.spec {
                            return self.extract_pod_resource_info(pod_spec);
                        }
                    }
                }
            }
            KubeWorkloadKind::Pod => {
                let api: kube::Api<Pod> = kube::Api::namespaced((**client).clone(), namespace);
                if let Ok(pod) = api.get(name).await {
                    if let Some(spec) = &pod.spec {
                        return self.extract_pod_resource_info(spec);
                    }
                }
            }
            KubeWorkloadKind::Job => {
                let api: kube::Api<Job> = kube::Api::namespaced((**client).clone(), namespace);
                if let Ok(job) = api.get(name).await {
                    if let Some(spec) = &job.spec {
                        if let Some(pod_spec) = &spec.template.spec {
                            return self.extract_pod_resource_info(pod_spec);
                        }
                    }
                }
            }
            KubeWorkloadKind::CronJob => {
                let api: kube::Api<CronJob> = kube::Api::namespaced((**client).clone(), namespace);
                if let Ok(cronjob) = api.get(name).await {
                    if let Some(spec) = &cronjob.spec {
                        if let Some(job_template) = &spec.job_template.spec {
                            if let Some(pod_spec) = &job_template.template.spec {
                                return self.extract_pod_resource_info(pod_spec);
                            }
                        }
                    }
                }
            }
            _ => {}
        }

        Ok(Vec::new())
    }

    fn extract_pod_resource_info(
        &self,
        pod_spec: &k8s_openapi::api::core::v1::PodSpec,
    ) -> Result<Vec<KubeContainerInfo>> {
        pod_spec
            .containers
            .iter()
            .map(|container| {
                let mut cpu_request = None;
                let mut cpu_limit = None;
                let mut memory_request = None;
                let mut memory_limit = None;

                if let Some(resources) = &container.resources {
                    // CPU and Memory requests
                    if let Some(requests) = &resources.requests {
                        if let Some(cpu_req) = requests.get("cpu") {
                            cpu_request = Some(cpu_req.0.clone());
                        }
                        if let Some(memory_req) = requests.get("memory") {
                            memory_request = Some(memory_req.0.clone());
                        }
                    }

                    // CPU and Memory limits
                    if let Some(limits) = &resources.limits {
                        if let Some(cpu_lim) = limits.get("cpu") {
                            cpu_limit = Some(cpu_lim.0.clone());
                        }
                        if let Some(memory_lim) = limits.get("memory") {
                            memory_limit = Some(memory_lim.0.clone());
                        }
                    }
                }

                // Error if container has no image
                let image = container.image.clone().ok_or_else(|| {
                    anyhow!("Container '{}' has no image specified", container.name)
                })?;

                Ok(KubeContainerInfo {
                    name: container.name.clone(),
                    image,
                    cpu_request,
                    cpu_limit,
                    memory_request,
                    memory_limit,
                    env_vars: vec![],
                })
            })
            .collect()
    }

    fn format_memory_bytes(bytes: u64) -> String {
        const KI: u64 = 1024;
        const MI: u64 = KI * 1024;
        const GI: u64 = MI * 1024;

        if bytes >= GI {
            format!("{}Gi", bytes / GI)
        } else if bytes >= MI {
            format!("{}Mi", bytes / MI)
        } else if bytes >= KI {
            format!("{}Ki", bytes / KI)
        } else {
            format!("{}B", bytes)
        }
    }
}

impl KubeManagerRpc for KubeManager {
    async fn get_workloads(
        self,
        _context: ::tarpc::context::Context,
        namespace: Option<String>,
        workload_kind_filter: Option<KubeWorkloadKind>,
        include_system_workloads: bool,
        pagination: Option<PaginationParams>,
    ) -> Result<KubeWorkloadList, String> {
        match self
            .collect_workloads(
                namespace,
                workload_kind_filter,
                include_system_workloads,
                pagination,
            )
            .await
        {
            Ok(workloads) => {
                tracing::info!(
                    "Successfully collected {} workloads",
                    workloads.workloads.len()
                );
                Ok(workloads)
            }
            Err(e) => {
                tracing::error!("Failed to collect workloads: {e}");
                Err(format!("Failed to collect workloads: {e}"))
            }
        }
    }

    async fn get_workload_details(
        self,
        _context: ::tarpc::context::Context,
        name: String,
        namespace: String,
    ) -> Result<Option<KubeWorkload>, String> {
        match self
            .get_workload_by_name(name.clone(), namespace.clone())
            .await
        {
            Ok(workload) => {
                if workload.is_some() {
                    tracing::info!("Successfully found workload: {namespace}/{name}");
                } else {
                    tracing::warn!("Workload not found: {namespace}/{name}");
                }
                Ok(workload)
            }
            Err(e) => {
                tracing::error!("Failed to get workload details for {namespace}/{name}: {e}");
                Err(format!("Failed to get workload details: {e}"))
            }
        }
    }

    async fn get_namespaces(
        self,
        _context: ::tarpc::context::Context,
    ) -> Result<Vec<KubeNamespace>, String> {
        match self.collect_namespaces().await {
            Ok(namespaces) => {
                tracing::info!("Successfully collected {} namespaces", namespaces.len());
                Ok(namespaces)
            }
            Err(e) => {
                tracing::error!("Failed to collect namespaces: {e}");
                Err(format!("Failed to collect namespaces: {e}"))
            }
        }
    }

    async fn get_workloads_yaml(
        self,
        _context: ::tarpc::context::Context,
        workloads: Vec<KubeAppCatalogWorkload>,
    ) -> Result<KubeWorkloadsWithResources, String> {
        match self.retrieve_workloads_yaml(workloads.clone()).await {
            Ok(workloads_with_resources) => {
                tracing::info!(
                    "Successfully retrieved {} workloads with {} services, {} configmaps, {} secrets",
                    workloads_with_resources.workloads.len(),
                    workloads_with_resources.services.len(),
                    workloads_with_resources.configmaps.len(),
                    workloads_with_resources.secrets.len()
                );
                Ok(workloads_with_resources)
            }
            Err(e) => {
                tracing::error!("Failed to retrieve workloads YAML: {}", e);
                Err(format!("Failed to retrieve workloads YAML: {}", e))
            }
        }
    }

    async fn deploy_workload_yaml(
        self,
        _context: ::tarpc::context::Context,
        namespace: String,
        workloads_with_resources: KubeWorkloadsWithResources,
        labels: std::collections::HashMap<String, String>,
    ) -> Result<(), String> {
        match self
            .apply_workloads_with_resources(namespace.clone(), workloads_with_resources, labels)
            .await
        {
            Ok(()) => {
                tracing::info!("Successfully deployed workloads to namespace: {namespace}");
                Ok(())
            }
            Err(e) => {
                tracing::error!("Failed to deploy workloads to namespace {namespace}: {e}");
                Err(format!("Failed to deploy workloads: {e}"))
            }
        }
    }

    async fn get_workloads_details(
        self,
        _context: ::tarpc::context::Context,
        workloads: Vec<WorkloadIdentifier>,
    ) -> Result<Vec<lapdev_common::kube::KubeWorkloadDetails>, String> {
        let mut details = Vec::new();

        for workload in workloads {
            match self
                .get_workload_resource_details(&workload.name, &workload.namespace, &workload.kind)
                .await
            {
                Ok(containers) => {
                    details.push(lapdev_common::kube::KubeWorkloadDetails {
                        name: workload.name,
                        namespace: workload.namespace,
                        kind: workload.kind,
                        containers,
                    });
                }
                Err(e) => {
                    tracing::error!(
                        "Failed to get workload details for {}/{}: {}",
                        workload.namespace,
                        workload.name,
                        e
                    );
                    return Err(format!(
                        "Failed to get workload details for {}/{}: {}",
                        workload.namespace, workload.name, e
                    ));
                }
            }
        }

        tracing::info!(
            "Successfully retrieved details for {} workloads",
            details.len()
        );
        Ok(details)
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

    #[test]
    fn test_extract_configmap_references_from_env() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      containers:
      - name: app
        image: nginx
        env:
        - name: CONFIG_VALUE
          valueFrom:
            configMapKeyRef:
              name: app-config
              key: config-key
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert_eq!(configmap_names, vec!["app-config"]);
    }

    #[test]
    fn test_extract_configmap_references_from_env_from() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      containers:
      - name: app
        image: nginx
        envFrom:
        - configMapRef:
            name: app-env-config
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert_eq!(configmap_names, vec!["app-env-config"]);
    }

    #[test]
    fn test_extract_configmap_references_from_volumes() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      volumes:
      - name: config-volume
        configMap:
          name: volume-config
      containers:
      - name: app
        image: nginx
        volumeMounts:
        - name: config-volume
          mountPath: /etc/config
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert_eq!(configmap_names, vec!["volume-config"]);
    }

    #[test]
    fn test_extract_configmap_references_from_init_containers() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      initContainers:
      - name: init-app
        image: busybox
        env:
        - name: INIT_CONFIG
          valueFrom:
            configMapKeyRef:
              name: init-config
              key: init-key
      containers:
      - name: app
        image: nginx
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert_eq!(configmap_names, vec!["init-config"]);
    }

    #[test]
    fn test_extract_configmap_references_from_pod_spec() {
        let pod_yaml = r#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
spec:
  initContainers:
  - name: init-app
    image: busybox
    envFrom:
    - configMapRef:
        name: pod-init-config
  containers:
  - name: app
    image: nginx
    env:
    - name: CONFIG_VALUE
      valueFrom:
        configMapKeyRef:
          name: pod-config
          key: config-key
"#;
        let pod: k8s_openapi::api::core::v1::Pod = serde_yaml::from_str(pod_yaml).unwrap();
        let pod_json = serde_json::to_value(&pod).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&pod_json);
        assert!(configmap_names.contains(&"pod-config".to_string()));
        assert!(configmap_names.contains(&"pod-init-config".to_string()));
        assert_eq!(configmap_names.len(), 2);
    }

    #[test]
    fn test_extract_configmap_references_multiple_sources() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      volumes:
      - name: config-volume
        configMap:
          name: volume-config
      initContainers:
      - name: init-app
        image: busybox
        env:
        - name: INIT_CONFIG
          valueFrom:
            configMapKeyRef:
              name: init-config
              key: init-key
        volumeMounts:
        - name: config-volume
          mountPath: /etc/init-config
      containers:
      - name: app
        image: nginx
        env:
        - name: CONFIG_VALUE
          valueFrom:
            configMapKeyRef:
              name: app-config
              key: config-key
        envFrom:
        - configMapRef:
            name: app-env-config
        volumeMounts:
        - name: config-volume
          mountPath: /etc/config
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert!(configmap_names.contains(&"volume-config".to_string()));
        assert!(configmap_names.contains(&"init-config".to_string()));
        assert!(configmap_names.contains(&"app-config".to_string()));
        assert!(configmap_names.contains(&"app-env-config".to_string()));
        assert_eq!(configmap_names.len(), 4);
    }

    #[test]
    fn test_extract_configmap_references_deduplication() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      volumes:
      - name: config-volume
        configMap:
          name: shared-config
      containers:
      - name: app1
        image: nginx
        env:
        - name: CONFIG_VALUE
          valueFrom:
            configMapKeyRef:
              name: shared-config
              key: config-key
        volumeMounts:
        - name: config-volume
          mountPath: /etc/config1
      - name: app2
        image: nginx
        envFrom:
        - configMapRef:
            name: shared-config
        volumeMounts:
        - name: config-volume
          mountPath: /etc/config2
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert_eq!(configmap_names, vec!["shared-config"]);
    }

    #[test]
    fn test_extract_configmap_references_no_references() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      containers:
      - name: app
        image: nginx
        env:
        - name: SIMPLE_VALUE
          value: "test"
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert!(configmap_names.is_empty());
    }

    #[test]
    fn test_extract_configmap_references_mixed_with_secrets() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      volumes:
      - name: config-volume
        configMap:
          name: volume-config
      - name: secret-volume
        secret:
          secretName: volume-secret
      containers:
      - name: app
        image: nginx
        env:
        - name: CONFIG_VALUE
          valueFrom:
            configMapKeyRef:
              name: app-config
              key: config-key
        - name: SECRET_VALUE
          valueFrom:
            secretKeyRef:
              name: app-secret
              key: secret-key
        volumeMounts:
        - name: config-volume
          mountPath: /etc/config
        - name: secret-volume
          mountPath: /etc/secrets
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let configmap_names = KubeManager::extract_configmap_references(&deployment_json);
        assert!(configmap_names.contains(&"volume-config".to_string()));
        assert!(configmap_names.contains(&"app-config".to_string()));
        assert_eq!(configmap_names.len(), 2);
        // Should not contain secrets
        assert!(!configmap_names.contains(&"volume-secret".to_string()));
        assert!(!configmap_names.contains(&"app-secret".to_string()));
    }

    #[test]
    fn test_extract_secret_references_from_env() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      containers:
      - name: app
        image: nginx
        env:
        - name: SECRET_VALUE
          valueFrom:
            secretKeyRef:
              name: app-secret
              key: secret-key
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert_eq!(secret_names, vec!["app-secret"]);
    }

    #[test]
    fn test_extract_secret_references_from_env_from() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      containers:
      - name: app
        image: nginx
        envFrom:
        - secretRef:
            name: app-env-secret
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert_eq!(secret_names, vec!["app-env-secret"]);
    }

    #[test]
    fn test_extract_secret_references_from_volumes() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      volumes:
      - name: secret-volume
        secret:
          secretName: volume-secret
      containers:
      - name: app
        image: nginx
        volumeMounts:
        - name: secret-volume
          mountPath: /etc/secrets
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert_eq!(secret_names, vec!["volume-secret"]);
    }

    #[test]
    fn test_extract_secret_references_from_image_pull_secrets() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      imagePullSecrets:
      - name: registry-secret
      containers:
      - name: app
        image: private-registry.com/app:latest
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert_eq!(secret_names, vec!["registry-secret"]);
    }

    #[test]
    fn test_extract_secret_references_from_init_containers() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      initContainers:
      - name: init-app
        image: busybox
        env:
        - name: INIT_SECRET
          valueFrom:
            secretKeyRef:
              name: init-secret
              key: init-key
      containers:
      - name: app
        image: nginx
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert_eq!(secret_names, vec!["init-secret"]);
    }

    #[test]
    fn test_extract_secret_references_from_pod_spec() {
        let pod_yaml = r#"
apiVersion: v1
kind: Pod
metadata:
  name: test-pod
spec:
  imagePullSecrets:
  - name: pod-registry-secret
  initContainers:
  - name: init-app
    image: busybox
    envFrom:
    - secretRef:
        name: pod-init-secret
  containers:
  - name: app
    image: nginx
    env:
    - name: SECRET_VALUE
      valueFrom:
        secretKeyRef:
          name: pod-secret
          key: secret-key
"#;
        let pod: k8s_openapi::api::core::v1::Pod = serde_yaml::from_str(pod_yaml).unwrap();
        let pod_json = serde_json::to_value(&pod).unwrap();

        let secret_names = KubeManager::extract_secret_references(&pod_json);
        assert!(secret_names.contains(&"pod-secret".to_string()));
        assert!(secret_names.contains(&"pod-init-secret".to_string()));
        assert!(secret_names.contains(&"pod-registry-secret".to_string()));
        assert_eq!(secret_names.len(), 3);
    }

    #[test]
    fn test_extract_secret_references_multiple_sources() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      imagePullSecrets:
      - name: registry-secret
      volumes:
      - name: secret-volume
        secret:
          secretName: volume-secret
      initContainers:
      - name: init-app
        image: busybox
        env:
        - name: INIT_SECRET
          valueFrom:
            secretKeyRef:
              name: init-secret
              key: init-key
        volumeMounts:
        - name: secret-volume
          mountPath: /etc/init-secrets
      containers:
      - name: app
        image: nginx
        env:
        - name: SECRET_VALUE
          valueFrom:
            secretKeyRef:
              name: app-secret
              key: secret-key
        envFrom:
        - secretRef:
            name: app-env-secret
        volumeMounts:
        - name: secret-volume
          mountPath: /etc/secrets
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert!(secret_names.contains(&"registry-secret".to_string()));
        assert!(secret_names.contains(&"volume-secret".to_string()));
        assert!(secret_names.contains(&"init-secret".to_string()));
        assert!(secret_names.contains(&"app-secret".to_string()));
        assert!(secret_names.contains(&"app-env-secret".to_string()));
        assert_eq!(secret_names.len(), 5);
    }

    #[test]
    fn test_extract_secret_references_deduplication() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      imagePullSecrets:
      - name: shared-secret
      volumes:
      - name: secret-volume
        secret:
          secretName: shared-secret
      containers:
      - name: app1
        image: nginx
        env:
        - name: SECRET_VALUE
          valueFrom:
            secretKeyRef:
              name: shared-secret
              key: secret-key
        volumeMounts:
        - name: secret-volume
          mountPath: /etc/secrets1
      - name: app2
        image: nginx
        envFrom:
        - secretRef:
            name: shared-secret
        volumeMounts:
        - name: secret-volume
          mountPath: /etc/secrets2
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert_eq!(secret_names, vec!["shared-secret"]);
    }

    #[test]
    fn test_extract_secret_references_no_references() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      containers:
      - name: app
        image: nginx
        env:
        - name: SIMPLE_VALUE
          value: "test"
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert!(secret_names.is_empty());
    }

    #[test]
    fn test_extract_secret_references_mixed_with_configmaps() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-app
spec:
  template:
    spec:
      volumes:
      - name: config-volume
        configMap:
          name: volume-config
      - name: secret-volume
        secret:
          secretName: volume-secret
      containers:
      - name: app
        image: nginx
        env:
        - name: CONFIG_VALUE
          valueFrom:
            configMapKeyRef:
              name: app-config
              key: config-key
        - name: SECRET_VALUE
          valueFrom:
            secretKeyRef:
              name: app-secret
              key: secret-key
        volumeMounts:
        - name: config-volume
          mountPath: /etc/config
        - name: secret-volume
          mountPath: /etc/secrets
"#;
        let deployment: k8s_openapi::api::apps::v1::Deployment =
            serde_yaml::from_str(deployment_yaml).unwrap();
        let deployment_json = serde_json::to_value(&deployment).unwrap();

        let secret_names = KubeManager::extract_secret_references(&deployment_json);
        assert!(secret_names.contains(&"volume-secret".to_string()));
        assert!(secret_names.contains(&"app-secret".to_string()));
        assert_eq!(secret_names.len(), 2);
        // Should not contain configmaps
        assert!(!secret_names.contains(&"volume-config".to_string()));
        assert!(!secret_names.contains(&"app-config".to_string()));
    }
}
