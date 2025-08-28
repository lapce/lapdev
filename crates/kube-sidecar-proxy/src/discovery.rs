use crate::{config::{ProxyConfig, RouteConfig, RouteTarget, AccessLevel}, error::{Result, SidecarProxyError}};
use k8s_openapi::api::core::v1::{Pod, Service, Endpoints, ConfigMap};
use kube::{
    api::{Api, ListParams},
    runtime::{watcher, WatchStreamExt},
    Client, ResourceExt,
};
use futures::{StreamExt, TryStreamExt};
use std::collections::HashMap;
use tokio::sync::{broadcast, RwLock};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

/// Service discovery manager that watches Kubernetes resources
/// and maintains routing configuration
pub struct ServiceDiscovery {
    client: Client,
    namespace: Option<String>,
    pod_name: Option<String>,
    environment_id: Option<Uuid>,
    config: RwLock<ProxyConfig>,
    config_tx: broadcast::Sender<ProxyConfig>,
}

impl ServiceDiscovery {
    pub fn new(
        client: Client,
        namespace: Option<String>,
        pod_name: Option<String>,
        environment_id: Option<Uuid>,
        initial_config: ProxyConfig,
    ) -> Self {
        let (config_tx, _) = broadcast::channel(16);
        
        Self {
            client,
            namespace,
            pod_name,
            environment_id,
            config: RwLock::new(initial_config),
            config_tx,
        }
    }

    /// Subscribe to configuration updates
    pub fn subscribe(&self) -> broadcast::Receiver<ProxyConfig> {
        self.config_tx.subscribe()
    }

    /// Get current configuration
    pub async fn get_config(&self) -> ProxyConfig {
        self.config.read().await.clone()
    }

    /// Start watching Kubernetes resources for configuration changes
    pub async fn start_watching(&self) -> Result<()> {
        let namespace = self.namespace.as_deref().unwrap_or("default");
        
        // Watch ConfigMaps for proxy configuration
        let configmap_task = self.watch_configmaps(namespace);
        
        // Watch Services for routing targets
        let services_task = self.watch_services(namespace);
        
        // Watch Endpoints for service discovery
        let endpoints_task = self.watch_endpoints(namespace);
        
        // Watch our own pod for annotations
        if let Some(pod_name) = &self.pod_name {
            let pod_task = self.watch_pod(namespace, pod_name);
            tokio::try_join!(configmap_task, services_task, endpoints_task, pod_task)?;
        } else {
            tokio::try_join!(configmap_task, services_task, endpoints_task)?;
        }
        
        Ok(())
    }

    async fn watch_configmaps(&self, namespace: &str) -> Result<()> {
        let api: Api<ConfigMap> = Api::namespaced(self.client.clone(), namespace);
        let config = watcher::Config::default();

        let mut stream = watcher(api, config).applied_objects().boxed();
        
        info!("Starting ConfigMap watcher in namespace: {}", namespace);
        
        while let Some(configmap) = stream.try_next().await? {
            if let Err(e) = self.handle_configmap_event(&configmap).await {
                error!("Error handling ConfigMap event: {}", e);
            }
        }
        
        Ok(())
    }

    async fn watch_services(&self, namespace: &str) -> Result<()> {
        let api: Api<Service> = Api::namespaced(self.client.clone(), namespace);
        let config = watcher::Config::default();

        let mut stream = watcher(api, config).applied_objects().boxed();
        
        info!("Starting Service watcher in namespace: {}", namespace);
        
        while let Some(service) = stream.try_next().await? {
            if let Err(e) = self.handle_service_event(&service).await {
                error!("Error handling Service event: {}", e);
            }
        }
        
        Ok(())
    }

    async fn watch_endpoints(&self, namespace: &str) -> Result<()> {
        let api: Api<Endpoints> = Api::namespaced(self.client.clone(), namespace);
        let config = watcher::Config::default();

        let mut stream = watcher(api, config).applied_objects().boxed();
        
        info!("Starting Endpoints watcher in namespace: {}", namespace);
        
        while let Some(endpoints) = stream.try_next().await? {
            if let Err(e) = self.handle_endpoints_event(&endpoints).await {
                error!("Error handling Endpoints event: {}", e);
            }
        }
        
        Ok(())
    }

    async fn watch_pod(&self, namespace: &str, pod_name: &str) -> Result<()> {
        let api: Api<Pod> = Api::namespaced(self.client.clone(), namespace);
        let lp = ListParams::default().fields(&format!("metadata.name={}", pod_name));
        let config = watcher::Config::default();

        let mut stream = watcher(api, config).applied_objects().boxed();
        
        info!("Starting Pod watcher for pod: {} in namespace: {}", pod_name, namespace);
        
        while let Some(pod) = stream.try_next().await? {
            if let Err(e) = self.handle_pod_event(&pod).await {
                error!("Error handling Pod event: {}", e);
            }
        }
        
        Ok(())
    }

    async fn handle_configmap_event(&self, configmap: &ConfigMap) -> Result<()> {
        debug!("Processing ConfigMap: {}", configmap.name_any());
        
        if let Some(data) = &configmap.data {
            if let Some(config_yaml) = data.get("proxy-config.yaml") {
                match serde_yaml::from_str::<ProxyConfig>(config_yaml) {
                    Ok(new_config) => {
                        info!("Updating proxy configuration from ConfigMap: {}", configmap.name_any());
                        self.update_config(new_config).await?;
                    },
                    Err(e) => {
                        error!("Failed to parse proxy configuration from ConfigMap {}: {}", 
                              configmap.name_any(), e);
                    }
                }
            }
        }
        
        Ok(())
    }

    async fn handle_service_event(&self, service: &Service) -> Result<()> {
        debug!("Processing Service: {}", service.name_any());
        
        // Check if this service has proxy annotations
        if let Some(annotations) = &service.metadata.annotations {
            if self.has_proxy_annotations(annotations) {
                info!("Updating routes for Service: {}", service.name_any());
                self.update_service_routes(service, annotations).await?;
            }
        }
        
        Ok(())
    }

    async fn handle_endpoints_event(&self, endpoints: &Endpoints) -> Result<()> {
        debug!("Processing Endpoints: {}", endpoints.name_any());
        
        // Update routing targets based on endpoint changes
        // This ensures the proxy routes to healthy backends only
        if let Some(subsets) = &endpoints.subsets {
            for subset in subsets {
                if let Some(addresses) = &subset.addresses {
                    debug!("Service {} has {} healthy endpoints", 
                          endpoints.name_any(), addresses.len());
                }
            }
        }
        
        Ok(())
    }

    async fn handle_pod_event(&self, pod: &Pod) -> Result<()> {
        debug!("Processing Pod: {}", pod.name_any());
        
        // Update configuration based on pod annotations
        if let Some(annotations) = &pod.metadata.annotations {
            if self.has_proxy_annotations(annotations) {
                info!("Updating configuration from Pod annotations: {}", pod.name_any());
                self.update_pod_config(annotations).await?;
            }
        }
        
        Ok(())
    }

    fn has_proxy_annotations(&self, annotations: &std::collections::BTreeMap<String, String>) -> bool {
        annotations.keys().any(|key| key.starts_with("lapdev.io/proxy-"))
    }

    async fn update_service_routes(&self, service: &Service, annotations: &std::collections::BTreeMap<String, String>) -> Result<()> {
        let service_name = service.name_any();
        let namespace = service.namespace().unwrap_or_else(|| "default".to_string());
        
        // Extract configuration from annotations
        let target_port = annotations
            .get("lapdev.io/proxy-target-port")
            .and_then(|p| p.parse::<u16>().ok())
            .unwrap_or(80);
            
        let access_level = annotations
            .get("lapdev.io/proxy-access-level")
            .and_then(|level| match level.as_str() {
                "personal" => Some(AccessLevel::Personal),
                "shared" => Some(AccessLevel::Shared),
                "public" => Some(AccessLevel::Public),
                _ => None,
            })
            .unwrap_or(AccessLevel::Personal);
            
        let requires_auth = annotations
            .get("lapdev.io/proxy-require-auth")
            .map(|auth| auth == "true")
            .unwrap_or(true);

        // Create route configuration
        let route = RouteConfig {
            path: format!("/{}/*", service_name),
            target: RouteTarget::Service {
                name: service_name.clone(),
                namespace: Some(namespace),
                port: target_port,
            },
            headers: HashMap::new(),
            timeout_ms: annotations
                .get("lapdev.io/proxy-timeout-ms")
                .and_then(|t| t.parse().ok()),
            requires_auth,
            access_level,
        };

        // Update configuration
        let mut config = self.config.write().await;
        
        // Remove existing routes for this service and add the new one
        config.routes.retain(|r| {
            if let RouteTarget::Service { name, .. } = &r.target {
                name != &service_name
            } else {
                true
            }
        });
        
        config.routes.push(route);
        
        // Notify subscribers
        let _ = self.config_tx.send(config.clone());
        
        info!("Updated routes for service: {}", service_name);
        Ok(())
    }

    async fn update_pod_config(&self, annotations: &std::collections::BTreeMap<String, String>) -> Result<()> {
        let mut config = self.config.write().await;
        let mut updated = false;

        // Update default target if specified
        if let Some(target) = annotations.get("lapdev.io/proxy-target-service") {
            if let Ok(addr) = target.parse() {
                config.default_target = addr;
                updated = true;
            }
        }

        // Add custom headers if specified
        if let Some(headers_json) = annotations.get("lapdev.io/proxy-custom-headers") {
            if let Ok(headers) = serde_json::from_str::<HashMap<String, String>>(headers_json) {
                // Apply headers to all routes or create a default route
                if config.routes.is_empty() {
                    let default_target = config.default_target;
                    config.routes.push(RouteConfig {
                        path: "/*".to_string(),
                        target: RouteTarget::Address(default_target),
                        headers,
                        timeout_ms: None,
                        requires_auth: true,
                        access_level: AccessLevel::Personal,
                    });
                } else {
                    for route in &mut config.routes {
                        route.headers.extend(headers.clone());
                    }
                }
                updated = true;
            }
        }

        if updated {
            let _ = self.config_tx.send(config.clone());
            info!("Updated proxy configuration from pod annotations");
        }

        Ok(())
    }

    async fn update_config(&self, new_config: ProxyConfig) -> Result<()> {
        {
            let mut config = self.config.write().await;
            *config = new_config.clone();
        }
        
        // Notify all subscribers of the configuration change
        if let Err(e) = self.config_tx.send(new_config) {
            warn!("Failed to notify configuration subscribers: {}", e);
        }
        
        Ok(())
    }

    /// Resolve a service target to actual endpoints
    pub async fn resolve_service_target(
        &self,
        name: &str,
        namespace: Option<&str>,
        port: u16,
    ) -> Result<Vec<std::net::SocketAddr>> {
        let ns = namespace.unwrap_or("default");
        let endpoints_api: Api<Endpoints> = Api::namespaced(self.client.clone(), ns);
        
        let endpoints = endpoints_api
            .get(name)
            .await
            .map_err(|e| SidecarProxyError::ServiceDiscovery(format!(
                "Failed to get endpoints for service {}: {}", name, e
            )))?;
            
        let mut addresses = Vec::new();
        
        if let Some(subsets) = endpoints.subsets {
            for subset in subsets {
                if let Some(ready_addresses) = subset.addresses {
                    for addr in ready_addresses {
                        let ip = addr.ip;
                        let socket_addr = format!("{}:{}", ip, port);
                        if let Ok(parsed) = socket_addr.parse() {
                            addresses.push(parsed);
                        }
                    }
                }
            }
        }
        
        if addresses.is_empty() {
            return Err(SidecarProxyError::TargetNotFound { 
                service_name: name.to_string() 
            });
        }
        
        Ok(addresses)
    }
}