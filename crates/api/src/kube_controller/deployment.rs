use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use k8s_openapi::api::apps::v1::{Deployment, DeploymentSpec};
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, EnvVarSource, ObjectFieldSelector, PodSpec, PodTemplateSpec,
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use lapdev_common::{
    kube::{
        KubeEnvironmentStatus, DEFAULT_SIDECAR_PROXY_BIND_ADDR, DEFAULT_SIDECAR_PROXY_METRICS_PORT,
        DEFAULT_SIDECAR_PROXY_PORT, SIDECAR_PROXY_BIND_ADDR_ENV_VAR,
        SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_PORT_ENV_VAR,
    },
    utils::resolve_api_host,
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
            for workload in workloads_with_resources.workloads.iter_mut() {
                if let KubeWorkloadYamlOnly::Deployment(yaml) = workload {
                    inject_sidecar_proxy_into_deployment_yaml(
                        yaml,
                        environment.id,
                        &environment.namespace,
                        &environment.auth_token,
                    )?;
                }
            }

            let status = KubeEnvironmentStatus::from_str(&environment.status)
                .unwrap_or(KubeEnvironmentStatus::Running);
            let desired_replicas = desired_devbox_proxy_replicas(status);
            let api_host = resolve_devbox_proxy_api_host();
            let devbox_proxy_yaml = build_devbox_proxy_deployment_yaml(
                &environment.namespace,
                environment.id,
                &environment.auth_token,
                environment.is_shared,
                desired_replicas,
                &api_host,
            )?;
            workloads_with_resources
                .workloads
                .push(KubeWorkloadYamlOnly::Deployment(devbox_proxy_yaml));
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
) -> Result<(), ApiError> {
    let mut deployment: Deployment = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse deployment YAML for sidecar injection: {}",
            err
        ))
    })?;

    if let Some(spec) = deployment.spec.as_mut() {
        if let Some(pod_spec) = spec.template.spec.as_mut() {
            ensure_sidecar_proxy_container(pod_spec, environment_id, namespace, auth_token);
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
    ));
}

fn build_sidecar_proxy_container(
    environment_id: Uuid,
    namespace: &str,
    auth_token: &str,
) -> Container {
    let sidecar_image = container_images::sidecar_proxy_image_reference();

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
                value: Some("lapdev-kube-manager.lapdev.svc:5001".to_string()),
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

fn desired_devbox_proxy_replicas(status: KubeEnvironmentStatus) -> i32 {
    match status {
        KubeEnvironmentStatus::Pausing | KubeEnvironmentStatus::Paused => 0,
        _ => 1,
    }
}

fn resolve_devbox_proxy_api_host() -> String {
    let host_env = std::env::var("LAPDEV_API_HOST").ok();
    resolve_api_host(host_env.as_deref())
}

fn build_devbox_proxy_deployment_yaml(
    namespace: &str,
    environment_id: Uuid,
    auth_token: &str,
    is_shared_environment: bool,
    replicas: i32,
    api_host: &str,
) -> Result<String, ApiError> {
    let mut selector_labels = BTreeMap::new();
    selector_labels.insert("app".to_string(), "lapdev-devbox-proxy".to_string());

    let metadata = ObjectMeta {
        name: Some("lapdev-devbox-proxy".to_string()),
        namespace: Some(namespace.to_string()),
        labels: Some(selector_labels.clone()),
        ..Default::default()
    };

    let template_metadata = ObjectMeta {
        labels: Some(selector_labels.clone()),
        ..Default::default()
    };

    let container = Container {
        name: "lapdev-devbox-proxy".to_string(),
        image: Some(container_images::devbox_proxy_image_reference()),
        env: Some(vec![
            EnvVar {
                name: "LAPDEV_API_HOST".to_string(),
                value: Some(api_host.to_string()),
                ..Default::default()
            },
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
                name: "IS_SHARED_ENVIRONMENT".to_string(),
                value: Some(is_shared_environment.to_string()),
                ..Default::default()
            },
        ]),
        ..Default::default()
    };

    let pod_spec = PodSpec {
        containers: vec![container],
        ..Default::default()
    };

    let template = PodTemplateSpec {
        metadata: Some(template_metadata),
        spec: Some(pod_spec),
    };

    let deployment_spec = DeploymentSpec {
        replicas: Some(replicas),
        selector: LabelSelector {
            match_labels: Some(selector_labels),
            ..Default::default()
        },
        template,
        ..Default::default()
    };

    let deployment = Deployment {
        metadata,
        spec: Some(deployment_spec),
        status: None,
    };

    serde_yaml::to_string(&deployment).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize devbox proxy deployment: {}",
            err
        ))
    })
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
    }

    #[test]
    fn desired_devbox_proxy_replicas_handles_pause_states() {
        assert_eq!(
            desired_devbox_proxy_replicas(KubeEnvironmentStatus::Running),
            1
        );
        assert_eq!(
            desired_devbox_proxy_replicas(KubeEnvironmentStatus::Pausing),
            0
        );
        assert_eq!(
            desired_devbox_proxy_replicas(KubeEnvironmentStatus::Paused),
            0
        );
        assert_eq!(
            desired_devbox_proxy_replicas(KubeEnvironmentStatus::ResumeFailed),
            1
        );
    }

    #[test]
    fn build_devbox_proxy_deployment_yaml_sets_expected_fields() {
        let environment_id = Uuid::new_v4();
        let yaml = build_devbox_proxy_deployment_yaml(
            "lapdev-namespace",
            environment_id,
            "auth-token",
            true,
            0,
            "api.example.dev",
        )
        .expect("deployment yaml should build");

        let deployment: Deployment =
            serde_yaml::from_str(&yaml).expect("yaml should deserialize into Deployment");
        assert_eq!(
            deployment.metadata.name.as_deref(),
            Some("lapdev-devbox-proxy")
        );
        assert_eq!(
            deployment.metadata.namespace.as_deref(),
            Some("lapdev-namespace")
        );

        let spec = deployment
            .spec
            .as_ref()
            .expect("deployment spec should be present");
        assert_eq!(spec.replicas, Some(0));

        let selector_labels = spec
            .selector
            .match_labels
            .as_ref()
            .expect("selector labels should be defined");
        assert_eq!(
            selector_labels.get("app"),
            Some(&"lapdev-devbox-proxy".to_string())
        );

        let pod_spec = spec
            .template
            .spec
            .as_ref()
            .expect("pod spec should be present");
        assert_eq!(pod_spec.containers.len(), 1);
        let container = &pod_spec.containers[0];
        assert_eq!(
            container.name,
            "lapdev-devbox-proxy".to_string(),
            "container should be named consistently"
        );
        let expected_image = super::container_images::devbox_proxy_image_reference();
        assert_eq!(
            container.image.as_deref(),
            Some(expected_image.as_str()),
            "devbox container should use shared repo tag"
        );

        let env_vars = container
            .env
            .as_ref()
            .expect("devbox container should have required env vars");
        let env_map: std::collections::HashMap<_, _> = env_vars
            .iter()
            .map(|var| (var.name.clone(), var.value.clone()))
            .collect();

        assert_eq!(
            env_map.get("LAPDEV_API_HOST"),
            Some(&Some("api.example.dev".to_string()))
        );
        assert_eq!(
            env_map.get("LAPDEV_ENVIRONMENT_ID"),
            Some(&Some(environment_id.to_string()))
        );
        assert_eq!(
            env_map.get("LAPDEV_ENVIRONMENT_AUTH_TOKEN"),
            Some(&Some("auth-token".to_string()))
        );
        assert_eq!(
            env_map.get("IS_SHARED_ENVIRONMENT"),
            Some(&Some("true".to_string()))
        );
    }
}
