use std::collections::HashMap;

use k8s_openapi::api::apps::v1::Deployment;
use lapdev_common::kube::ProxyPortRoute;
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::KubeWorkloadYamlOnly;
use lapdev_rpc::error::ApiError;
use uuid::Uuid;

use super::{resources, KubeController};

impl KubeController {
    pub(super) async fn deploy_environment_resources(
        &self,
        target_server: &KubeClusterServer,
        environment: &lapdev_db_entities::kube_environment::Model,
        workloads_with_resources: lapdev_kube_rpc::KubeWorkloadsWithResources,
        extra_labels: Option<HashMap<String, String>>,
    ) -> Result<(), ApiError> {
        let mut workloads_with_resources = workloads_with_resources;
        let namespace = &environment.namespace;
        let environment_name = &environment.name;

        tracing::info!(
            "Deploying environment resources for '{}' in namespace '{}'",
            environment_name,
            namespace
        );

        if workloads_with_resources.workloads.is_empty() {
            tracing::warn!("No workloads found for environment '{}'", environment_name);
            return Ok(());
        }

        tracing::info!(
            "Found {} workloads to deploy for environment '{}'",
            workloads_with_resources.workloads.len(),
            environment_name
        );

        if environment.base_environment_id.is_none() {
            let manager_namespace = self
                .db
                .get_kube_cluster(environment.cluster_id)
                .await
                .map_err(ApiError::from)?
                .and_then(|cluster| cluster.manager_namespace);
            let manager_namespace_ref = manager_namespace.as_deref();

            for workload in workloads_with_resources.workloads.iter_mut() {
                if let KubeWorkloadYamlOnly::Deployment(yaml) = workload {
                    inject_sidecar_proxy_into_deployment_yaml(
                        yaml,
                        environment.id,
                        &environment.namespace,
                        &environment.auth_token,
                        manager_namespace_ref,
                        None,
                    )?;
                }
            }
        }

        // Prepare environment-specific labels
        let mut environment_labels = std::collections::HashMap::new();
        environment_labels.insert(
            "lapdev.environment".to_string(),
            environment_name.to_string(),
        );
        environment_labels.insert("lapdev.managed-by".to_string(), "lapdev".to_string());
        if environment.base_environment_id.is_some() {
            environment_labels.insert(
                "lapdev.io/branch-environment-id".to_string(),
                environment.id.to_string(),
            );
        }
        if let Some(extra) = extra_labels {
            for (key, value) in extra {
                environment_labels.insert(key, value);
            }
        }

        // Deploy all workloads and resources in a single call
        if environment.auth_token.is_empty() {
            return Err(ApiError::InvalidRequest(
                "Environment auth token is required".to_string(),
            ));
        }
        match target_server
            .rpc_client
            .deploy_workload_yaml(
                tarpc::context::current(),
                namespace.to_string(),
                workloads_with_resources,
                environment_labels,
            )
            .await
        {
            Ok(Ok(())) => {
                tracing::info!("Successfully deployed all workloads to target cluster");
                Ok(())
            }
            Ok(Err(e)) => {
                tracing::error!("Failed to deploy workloads to target cluster: {}", e);
                Err(ApiError::InvalidRequest(format!(
                    "Failed to deploy workloads to target cluster: {e}"
                )))
            }
            Err(e) => Err(ApiError::InvalidRequest(format!(
                "Connection error to target cluster: {e}"
            ))),
        }
    }
}

pub(super) fn inject_sidecar_proxy_into_deployment_yaml(
    yaml: &mut String,
    environment_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_namespace: Option<&str>,
    proxy_routes: Option<&[ProxyPortRoute]>,
) -> Result<(), ApiError> {
    let mut deployment: Deployment = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse deployment YAML for sidecar injection: {}",
            err
        ))
    })?;

    let routes = proxy_routes.unwrap_or_default();
    let options = resources::SidecarInjectionOptions {
        environment_id,
        namespace,
        auth_token,
        manager_namespace,
        proxy_routes: routes,
    };

    resources::inject_sidecar_proxy_into_deployment(&mut deployment, &options).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to inject sidecar proxy into deployment manifest: {}",
            err
        ))
    })?;

    *yaml = serde_yaml::to_string(&deployment).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize deployment YAML after sidecar injection: {}",
            err
        ))
    })?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use k8s_openapi::api::apps::v1::DeploymentSpec;
    use k8s_openapi::api::core::v1::{Container, PodSpec, PodTemplateSpec};
    use k8s_openapi::apimachinery::pkg::apis::meta::v1::LabelSelector;
    use lapdev_common::kube::{
        DEFAULT_SIDECAR_PROXY_BIND_ADDR, DEFAULT_SIDECAR_PROXY_PORT,
        SIDECAR_PROXY_BIND_ADDR_ENV_VAR, SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_PORT_ENV_VAR,
    };
    use super::super::container_images;

    #[test]
    fn inject_sidecar_proxy_appends_once() {
        let mut deployment = Deployment {
            metadata: Default::default(),
            spec: Some(DeploymentSpec {
                replicas: None,
                selector: LabelSelector::default(),
                template: PodTemplateSpec {
                    metadata: Default::default(),
                    spec: Some(PodSpec {
                        containers: vec![Container {
                            name: "primary".to_string(),
                            ..Default::default()
                        }],
                        ..Default::default()
                    }),
                },
                ..Default::default()
            }),
            status: None,
        };

        let environment_id = Uuid::new_v4();
        let options = resources::SidecarInjectionOptions {
            environment_id,
            namespace: "lapdev-namespace",
            auth_token: "auth-token",
            manager_namespace: Some("lapdev"),
            proxy_routes: &[],
        };

        resources::inject_sidecar_proxy_into_deployment(&mut deployment, &options).unwrap();
        resources::inject_sidecar_proxy_into_deployment(&mut deployment, &options).unwrap();

        let pod_spec = deployment
            .spec
            .as_ref()
            .and_then(|spec| spec.template.spec.as_ref())
            .expect("deployment pod spec should exist");

        let sidecars: Vec<&Container> = pod_spec
            .containers
            .iter()
            .filter(|container| container.name == "lapdev-sidecar-proxy")
            .collect();
        assert_eq!(sidecars.len(), 1, "sidecar should be appended exactly once");

        let sidecar = sidecars[0];
        let expected_image = container_images::sidecar_proxy_image_reference();
        assert_eq!(
            sidecar.image.as_deref(),
            Some(expected_image.as_str()),
            "sidecar should use the shared repo:tag reference"
        );
        assert!(
            sidecar.args.is_none(),
            "sidecar should not rely on CLI arguments once injected"
        );

        let env_vars = sidecar
            .env
            .as_ref()
            .expect("sidecar should include required environment variables");
        let bind_addr_var = env_vars
            .iter()
            .find(|var| var.name == SIDECAR_PROXY_BIND_ADDR_ENV_VAR)
            .expect("bind address environment variable should be set");
        assert_eq!(
            bind_addr_var.value.as_deref(),
            Some(DEFAULT_SIDECAR_PROXY_BIND_ADDR),
            "bind address env var should carry the default proxy address"
        );

        let port_var = env_vars
            .iter()
            .find(|var| var.name == SIDECAR_PROXY_PORT_ENV_VAR)
            .expect("port environment variable should be set");
        let expected_port = DEFAULT_SIDECAR_PROXY_PORT.to_string();
        assert_eq!(
            port_var.value.as_deref(),
            Some(expected_port.as_str()),
            "port env var should reflect the default proxy port"
        );

        let manager_addr_var = env_vars
            .iter()
            .find(|var| var.name == SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR)
            .expect("manager addr environment variable should be set");
        assert_eq!(
            manager_addr_var.value.as_deref(),
            Some("lapdev-kube-manager.lapdev.svc:5001"),
            "manager addr env var should default to lapdev namespace"
        );
    }
}
