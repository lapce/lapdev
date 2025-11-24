use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use anyhow::{anyhow, Result};
use futures::{pin_mut, TryStreamExt};
use k8s_openapi::NamespaceResourceScope;
use kube::{
    runtime::watcher::{self, watcher, Error as WatcherError, Event},
    Api, Resource, ResourceExt,
};
use lapdev_kube_rpc::{
    KubeClusterRpcClient, ResourceChangeEvent, ResourceChangeType, ResourceType,
};
use serde::de::DeserializeOwned;
use tokio::{sync::RwLock, task::JoinHandle, time::sleep};
use tracing::{debug, error, info, warn};

const WATCHED_RESOURCE_TYPES: &[ResourceType] = &[
    ResourceType::Deployment,
    ResourceType::StatefulSet,
    ResourceType::DaemonSet,
    ResourceType::ReplicaSet,
    ResourceType::Job,
    ResourceType::CronJob,
    ResourceType::ConfigMap,
    ResourceType::Secret,
    ResourceType::Service,
];

type WatchKey = (ResourceType, String);

#[derive(Clone)]
pub struct WatchManager {
    kube_client: Arc<kube::Client>,
    rpc_client: Arc<RwLock<Option<KubeClusterRpcClient>>>,
    namespaces: Arc<RwLock<HashSet<String>>>,
    tasks: Arc<RwLock<HashMap<WatchKey, JoinHandle<()>>>>,
}

impl WatchManager {
    pub fn new(kube_client: Arc<kube::Client>) -> Self {
        Self {
            kube_client,
            rpc_client: Arc::new(RwLock::new(None)),
            namespaces: Arc::new(RwLock::new(HashSet::new())),
            tasks: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub async fn set_rpc_client(&self, client: KubeClusterRpcClient) {
        let mut guard = self.rpc_client.write().await;
        *guard = Some(client);
    }

    pub async fn clear_rpc_client(&self) {
        let mut guard = self.rpc_client.write().await;
        *guard = None;
    }

    pub async fn configure_watches(&self, namespaces: Vec<String>) -> Result<()> {
        let new_namespaces: HashSet<String> = namespaces
            .into_iter()
            .map(|ns| ns.trim().to_string())
            .filter(|ns| !ns.is_empty())
            .collect();

        let (to_remove, to_add) = {
            let current_namespaces = self.namespaces.read().await;
            let to_remove: Vec<String> = current_namespaces
                .iter()
                .filter(|ns| !new_namespaces.contains(*ns))
                .cloned()
                .collect();
            let to_add: Vec<String> = new_namespaces
                .iter()
                .filter(|ns| !current_namespaces.contains(*ns))
                .cloned()
                .collect();
            (to_remove, to_add)
        };

        for namespace in &to_remove {
            self.stop_namespace(namespace).await;
        }

        for namespace in &to_add {
            self.start_namespace(namespace.clone()).await;
        }

        let mut current_namespaces = self.namespaces.write().await;
        *current_namespaces = new_namespaces;

        Ok(())
    }

    pub async fn add_namespace_watch(&self, namespace: String) -> Result<()> {
        let namespace = namespace.trim().to_string();
        if namespace.is_empty() {
            return Ok(());
        }

        {
            let namespaces = self.namespaces.read().await;
            if namespaces.contains(&namespace) {
                return Ok(());
            }
        }

        self.start_namespace(namespace.clone()).await;

        let mut namespaces = self.namespaces.write().await;
        namespaces.insert(namespace);
        Ok(())
    }

    pub async fn remove_namespace_watch(&self, namespace: String) -> Result<()> {
        let namespace = namespace.trim().to_string();
        if namespace.is_empty() {
            return Ok(());
        }

        let removed = {
            let mut namespaces = self.namespaces.write().await;
            namespaces.remove(&namespace)
        };

        if removed {
            self.stop_namespace(&namespace).await;
        }

        Ok(())
    }

    async fn stop_namespace(&self, namespace: &str) {
        let mut tasks = self.tasks.write().await;
        let keys: Vec<WatchKey> = tasks
            .keys()
            .filter(|(_, ns)| ns.as_str() == namespace)
            .cloned()
            .collect();

        for key in keys {
            if let Some(handle) = tasks.remove(&key) {
                handle.abort();
                debug!(
                    namespace = namespace,
                    resource_type = ?key.0,
                    "Stopped watcher for namespace"
                );
            }
        }
    }

    async fn start_namespace(&self, namespace: String) {
        let mut tasks_guard = self.tasks.write().await;

        for rt in WATCHED_RESOURCE_TYPES {
            let resource_type = *rt;
            let key = (resource_type, namespace.clone());

            if tasks_guard.contains_key(&key) {
                continue;
            }

            let kube_client = self.kube_client.clone();
            let rpc_client = self.rpc_client.clone();
            let namespace_clone = namespace.clone();

            let handle = tokio::spawn(async move {
                info!(
                    namespace = namespace_clone.as_str(),
                    resource_type = ?resource_type,
                    "Starting watcher task"
                );
                let mut backoff = Duration::from_secs(1);
                loop {
                    let result = watch_resource(
                        kube_client.clone(),
                        rpc_client.clone(),
                        namespace_clone.clone(),
                        resource_type,
                    )
                    .await;

                    match result {
                        Ok(()) => {
                            info!(
                                namespace = namespace_clone.as_str(),
                                resource_type = ?resource_type,
                                "Watcher completed gracefully, restarting"
                            );
                            backoff = Duration::from_secs(1);
                        }
                        Err(err) => {
                            warn!(
                                namespace = namespace_clone.as_str(),
                                resource_type = ?resource_type,
                                error = ?err,
                                "Watcher encountered error, restarting with backoff"
                            );
                            sleep(backoff).await;
                            backoff = (backoff * 2).min(Duration::from_secs(30));
                        }
                    }
                }
            });

            tasks_guard.insert(key, handle);
        }
    }
}

async fn watch_resource(
    kube_client: Arc<kube::Client>,
    rpc_client: Arc<RwLock<Option<KubeClusterRpcClient>>>,
    namespace: String,
    resource_type: ResourceType,
) -> Result<()> {
    match resource_type {
        ResourceType::Deployment => {
            watch_kind::<k8s_openapi::api::apps::v1::Deployment>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::StatefulSet => {
            watch_kind::<k8s_openapi::api::apps::v1::StatefulSet>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::DaemonSet => {
            watch_kind::<k8s_openapi::api::apps::v1::DaemonSet>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::ReplicaSet => {
            watch_kind::<k8s_openapi::api::apps::v1::ReplicaSet>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::Job => {
            watch_kind::<k8s_openapi::api::batch::v1::Job>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::CronJob => {
            watch_kind::<k8s_openapi::api::batch::v1::CronJob>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::ConfigMap => {
            watch_kind::<k8s_openapi::api::core::v1::ConfigMap>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::Secret => {
            watch_kind::<k8s_openapi::api::core::v1::Secret>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
        ResourceType::Service => {
            watch_kind::<k8s_openapi::api::core::v1::Service>(
                kube_client,
                rpc_client,
                namespace,
                resource_type,
            )
            .await
        }
    }
}

async fn watch_kind<K>(
    kube_client: Arc<kube::Client>,
    rpc_client: Arc<RwLock<Option<KubeClusterRpcClient>>>,
    namespace: String,
    resource_type: ResourceType,
) -> Result<()>
where
    K: Clone
        + std::fmt::Debug
        + DeserializeOwned
        + Resource<Scope = NamespaceResourceScope>
        + Send
        + Sync
        + 'static
        + serde::Serialize,
    <K as Resource>::DynamicType:
        Default + Eq + std::hash::Hash + Clone + std::fmt::Debug + Send + Sync,
{
    let api: Api<K> = Api::namespaced(kube_client.as_ref().clone(), &namespace);
    let watcher_stream = watcher(api, watcher::Config::default().timeout(60));
    pin_mut!(watcher_stream);
    let mut seen_versions: HashMap<String, String> = HashMap::new();

    while let Some(event) = watcher_stream.try_next().await.map_err(map_watcher_error)? {
        match event {
            Event::Apply(obj) => {
                if let Some(change_event) = build_event(
                    &namespace,
                    resource_type,
                    ResourceChangeType::Created,
                    &mut seen_versions,
                    obj,
                ) {
                    send_event(rpc_client.clone(), change_event).await;
                }
            }
            Event::Delete(obj) => {
                if let Some(change_event) = build_event(
                    &namespace,
                    resource_type,
                    ResourceChangeType::Deleted,
                    &mut seen_versions,
                    obj,
                ) {
                    send_event(rpc_client.clone(), change_event).await;
                }
            }
            Event::Init => {
                debug!(
                    namespace = namespace.as_str(),
                    resource_type = ?resource_type,
                    "Watcher received init signal"
                );
            }
            Event::InitApply(obj) => {
                if let Some(change_event) = build_event(
                    &namespace,
                    resource_type,
                    ResourceChangeType::Created,
                    &mut seen_versions,
                    obj,
                ) {
                    send_event(rpc_client.clone(), change_event).await;
                }
            }
            Event::InitDone => {
                debug!(
                    namespace = namespace.as_str(),
                    resource_type = ?resource_type,
                    "Watcher initial sync completed"
                );
            }
        }
    }

    Ok(())
}

fn map_watcher_error(err: WatcherError) -> anyhow::Error {
    match err {
        WatcherError::InitialListFailed(source) => anyhow!("initial list request failed: {source}"),
        WatcherError::WatchStartFailed(source) => anyhow!("failed to start watcher: {source}"),
        WatcherError::WatchError(source) => anyhow!("watch error from API server: {source}"),
        WatcherError::WatchFailed(source) => anyhow!("watch stream failed: {source}"),
        WatcherError::NoResourceVersion => {
            anyhow!("watch event missing resourceVersion (resource may not support watch)")
        }
    }
}
fn build_event<K>(
    namespace: &str,
    resource_type: ResourceType,
    initial_change_type: ResourceChangeType,
    seen_versions: &mut HashMap<String, String>,
    obj: K,
) -> Option<ResourceChangeEvent>
where
    K: ResourceExt + serde::Serialize,
{
    let name = obj.name_any();
    if name.is_empty() {
        warn!(
            namespace = namespace,
            resource_type = ?resource_type,
            "Skipping resource with empty name"
        );
        return None;
    }

    let resource_version = match obj.resource_version() {
        Some(rv) => rv,
        None => {
            warn!(
                namespace = namespace,
                resource = name,
                resource_type = ?resource_type,
                "Skipping resource without resourceVersion"
            );
            return None;
        }
    };

    let change_type = match initial_change_type {
        ResourceChangeType::Created => {
            if seen_versions.contains_key(&name) {
                ResourceChangeType::Updated
            } else {
                ResourceChangeType::Created
            }
        }
        ResourceChangeType::Updated => {
            if seen_versions
                .get(&name)
                .map(|prev| prev == &resource_version)
                == Some(true)
            {
                return None;
            }
            ResourceChangeType::Updated
        }
        ResourceChangeType::Deleted => ResourceChangeType::Deleted,
    };

    match change_type {
        ResourceChangeType::Deleted => {
            seen_versions.remove(&name);
        }
        _ => {
            seen_versions.insert(name.clone(), resource_version.clone());
        }
    }

    let resource_yaml = if should_include_yaml(resource_type) {
        match serde_yaml::to_string(&obj) {
            Ok(yaml) => Some(yaml),
            Err(err) => {
                warn!(
                    namespace = namespace,
                    resource = name,
                    resource_type = ?resource_type,
                    error = ?err,
                    "Failed to serialize resource to YAML"
                );
                None
            }
        }
    } else {
        None
    };

    Some(ResourceChangeEvent {
        namespace: namespace.to_string(),
        resource_type,
        resource_name: name,
        change_type,
        resource_version,
        resource_yaml,
        timestamp: chrono::Utc::now(),
    })
}

fn should_include_yaml(resource_type: ResourceType) -> bool {
    matches!(
        resource_type,
        ResourceType::Deployment
            | ResourceType::StatefulSet
            | ResourceType::DaemonSet
            | ResourceType::ReplicaSet
            | ResourceType::Job
            | ResourceType::CronJob
            | ResourceType::Service
    )
}

async fn send_event(
    rpc_client_store: Arc<RwLock<Option<KubeClusterRpcClient>>>,
    event: ResourceChangeEvent,
) {
    let client = {
        let guard = rpc_client_store.read().await;
        guard.clone()
    };

    let Some(client) = client else {
        debug!(
            namespace = event.namespace.as_str(),
            resource_type = ?event.resource_type,
            resource_name = event.resource_name.as_str(),
            "RPC client not available, skipping resource change event"
        );
        return;
    };

    match client
        .report_resource_change(tarpc::context::current(), event.clone())
        .await
    {
        Ok(_) => {
            debug!(
                namespace = event.namespace.as_str(),
                resource_type = ?event.resource_type,
                resource_name = event.resource_name.as_str(),
                change_type = ?event.change_type,
                "Sent resource change event"
            );
        }
        Err(err) => {
            error!(
                namespace = event.namespace.as_str(),
                resource_type = ?event.resource_type,
                resource_name = event.resource_name.as_str(),
                change_type = ?event.change_type,
                error = ?err,
                "Failed to send resource change event"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::Deployment;
    use serde_yaml::Value;
    use std::collections::HashMap;

    #[test]
    fn build_event_preserves_status_ready_replicas_in_yaml() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: default
  resourceVersion: "123"
spec:
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
        - name: web
          image: nginx:1.27
status:
  readyReplicas: 1
"#;
        let deployment: Deployment = serde_yaml::from_str(deployment_yaml).unwrap();
        let mut seen_versions = HashMap::new();

        let event = build_event(
            "default",
            ResourceType::Deployment,
            ResourceChangeType::Created,
            &mut seen_versions,
            deployment,
        )
        .expect("expected change event");

        assert_eq!(event.resource_name, "web");
        assert_eq!(event.change_type, ResourceChangeType::Created);

        let yaml = event
            .resource_yaml
            .expect("expected serialized resource YAML in change event");
        let parsed: Value = serde_yaml::from_str(&yaml).expect("YAML should parse");

        let ready_replicas = parsed
            .get("status")
            .and_then(|status| status.get("readyReplicas"))
            .and_then(Value::as_i64);

        assert_eq!(
            ready_replicas,
            Some(1),
            "readyReplicas should round-trip in YAML"
        );
    }

    #[test]
    fn build_event_provides_yaml_for_deleted_workloads() {
        let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: default
  resourceVersion: "123"
  labels:
    lapdev.io/branch-environment-id: "abc"
spec:
  selector:
    matchLabels:
      app: web
  template:
    metadata:
      labels:
        app: web
    spec:
      containers:
        - name: web
          image: nginx:1.27
"#;
        let deployment: Deployment = serde_yaml::from_str(deployment_yaml).unwrap();
        let mut seen_versions = HashMap::new();

        let event = build_event(
            "default",
            ResourceType::Deployment,
            ResourceChangeType::Deleted,
            &mut seen_versions,
            deployment,
        )
        .expect("expected delete event");

        assert_eq!(event.change_type, ResourceChangeType::Deleted);
        assert!(
            event.resource_yaml.is_some(),
            "deleted events should still include serialized YAML"
        );
    }
}
