use std::collections::HashMap;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, EnvVarSource, ObjectFieldSelector, PodSpec,
};
use lapdev_common::kube::{
    DEFAULT_SIDECAR_PROXY_BIND_ADDR, DEFAULT_SIDECAR_PROXY_METRICS_PORT,
    DEFAULT_SIDECAR_PROXY_PORT, SIDECAR_PROXY_BIND_ADDR_ENV_VAR,
    SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_PORT_ENV_VAR,
};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::KubeWorkloadYamlOnly;
use lapdev_rpc::error::ApiError;
use uuid::Uuid;

use super::{container_images, KubeController};

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

fn inject_sidecar_proxy_into_deployment_yaml(
    yaml: &mut String,
    environment_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_namespace: Option<&str>,
) -> Result<(), ApiError> {
    let mut deployment: Deployment = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse deployment YAML for sidecar injection: {}",
            err
        ))
    })?;

    if let Some(spec) = deployment.spec.as_mut() {
        if let Some(pod_spec) = spec.template.spec.as_mut() {
            ensure_sidecar_proxy_container(
                pod_spec,
                environment_id,
                namespace,
                auth_token,
                manager_namespace,
            );
        }
    }

    *yaml = serde_yaml::to_string(&deployment).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize deployment YAML after sidecar injection: {}",
            err
        ))
    })?;

    Ok(())
}

fn ensure_sidecar_proxy_container(
    pod_spec: &mut PodSpec,
    environment_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_namespace: Option<&str>,
) {
    let already_present = pod_spec
        .containers
        .iter()
        .any(|container| container.name == "lapdev-sidecar-proxy");
    if already_present {
        return;
    }

    pod_spec.containers.push(build_sidecar_proxy_container(
        environment_id,
        namespace,
        auth_token,
        manager_namespace,
    ));
}

fn build_sidecar_proxy_container(
    environment_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_namespace: Option<&str>,
) -> Container {
    let sidecar_image = container_images::sidecar_proxy_image_reference();
    let manager_namespace = manager_namespace.unwrap_or("lapdev");
    let manager_addr = format!("lapdev-kube-manager.{manager_namespace}.svc:5001");

    Container {
        name: "lapdev-sidecar-proxy".to_string(),
        image: Some(sidecar_image),
        ports: Some(vec![
            ContainerPort {
                container_port: DEFAULT_SIDECAR_PROXY_PORT as i32,
                name: Some("proxy".to_string()),
                protocol: Some("TCP".to_string()),
                ..Default::default()
            },
            ContainerPort {
                container_port: DEFAULT_SIDECAR_PROXY_METRICS_PORT as i32,
                name: Some("metrics".to_string()),
                protocol: Some("TCP".to_string()),
                ..Default::default()
            },
        ]),
        env: Some(vec![
            EnvVar {
                name: "LAPDEV_ENVIRONMENT_ID".to_string(),
                value: Some(environment_id.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "LAPDEV_ENVIRONMENT_AUTH_TOKEN".to_string(),
                value: Some(auth_token.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: "KUBERNETES_NAMESPACE".to_string(),
                value: Some(namespace.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: SIDECAR_PROXY_BIND_ADDR_ENV_VAR.to_string(),
                value: Some(DEFAULT_SIDECAR_PROXY_BIND_ADDR.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: SIDECAR_PROXY_PORT_ENV_VAR.to_string(),
                value: Some(DEFAULT_SIDECAR_PROXY_PORT.to_string()),
                ..Default::default()
            },
            EnvVar {
                name: SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR.to_string(),
                value: Some(manager_addr),
                ..Default::default()
            },
            EnvVar {
                name: "HOSTNAME".to_string(),
                value_from: Some(EnvVarSource {
                    field_ref: Some(ObjectFieldSelector {
                        field_path: "metadata.name".to_string(),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
                ..Default::default()
            },
        ]),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ensure_sidecar_proxy_container_appends_once() {
        let mut pod_spec = PodSpec {
            containers: vec![Container {
                name: "primary".to_string(),
                ..Default::default()
            }],
            ..Default::default()
        };

        let environment_id = Uuid::new_v4();
        ensure_sidecar_proxy_container(
            &mut pod_spec,
            environment_id,
            "lapdev-namespace",
            "auth-token",
            Some("lapdev"),
        );

        let mut sidecars: Vec<&Container> = pod_spec
            .containers
            .iter()
            .filter(|container| container.name == "lapdev-sidecar-proxy")
            .collect();
        assert_eq!(sidecars.len(), 1, "sidecar should be appended exactly once");

        ensure_sidecar_proxy_container(
            &mut pod_spec,
            environment_id,
            "lapdev-namespace",
            "auth-token",
            Some("lapdev"),
        );

        sidecars = pod_spec
            .containers
            .iter()
            .filter(|container| container.name == "lapdev-sidecar-proxy")
            .collect();
        assert_eq!(
            sidecars.len(),
            1,
            "subsequent injections must not duplicate the sidecar"
        );

        let sidecar = sidecars.pop().expect("sidecar container should exist");
        let expected_image = super::container_images::sidecar_proxy_image_reference();
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
