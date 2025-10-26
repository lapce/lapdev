use std::collections::HashMap;

use k8s_openapi::api::apps::v1::Deployment;
use k8s_openapi::api::core::v1::{
    Container, ContainerPort, EnvVar, EnvVarSource, ObjectFieldSelector,
};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::KubeWorkloadYamlOnly;
use lapdev_rpc::error::ApiError;
use uuid::Uuid;

use super::KubeController;

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
        if let Some(template) = spec.template.spec.as_mut() {
            let already_present = template
                .containers
                .iter()
                .any(|container| container.name == "lapdev-sidecar-proxy");
            if !already_present {
                let sidecar_container = Container {
                    name: "lapdev-sidecar-proxy".to_string(),
                    image: Some("lapdev/kube-sidecar-proxy:latest".to_string()),
                    ports: Some(vec![
                        ContainerPort {
                            container_port: 8080,
                            name: Some("proxy".to_string()),
                            protocol: Some("TCP".to_string()),
                            ..Default::default()
                        },
                        ContainerPort {
                            container_port: 9090,
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
                    args: Some(vec![
                        "--listen-addr".to_string(),
                        "0.0.0.0:8080".to_string(),
                        "--target-addr".to_string(),
                        "127.0.0.1:3000".to_string(),
                    ]),
                    ..Default::default()
                };
                template.containers.push(sidecar_container);
            }
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
