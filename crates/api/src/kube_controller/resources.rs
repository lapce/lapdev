use std::collections::HashMap;

use anyhow::{anyhow, Result};
use k8s_openapi::{
    api::{
        apps::v1::{
            DaemonSet, DaemonSetSpec, Deployment, DeploymentSpec, ReplicaSet, ReplicaSetSpec,
            StatefulSet, StatefulSetSpec,
        },
        batch::v1::{CronJob, CronJobSpec, Job, JobSpec},
        core::v1::{
            ConfigMap, Container, ContainerPort, EnvVar, EnvVarSource, ObjectFieldSelector, Pod,
            PodSpec, PodTemplateSpec, Secret, Service,
        },
    },
    apimachinery::pkg::{api::resource::Quantity, apis::meta::v1::ObjectMeta},
};
use lapdev_common::kube::{
    KubeContainerImage, KubeContainerInfo, KubeWorkloadKind, ProxyPortRoute,
    DEFAULT_SIDECAR_PROXY_BIND_ADDR, DEFAULT_SIDECAR_PROXY_METRICS_PORT,
    DEFAULT_SIDECAR_PROXY_PORT, SIDECAR_PROXY_BIND_ADDR_ENV_VAR,
    SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR, SIDECAR_PROXY_PORT_ENV_VAR, SIDECAR_PROXY_WORKLOAD_ENV_VAR,
};
use lapdev_kube_rpc::KubeWorkloadYamlOnly;
use serde_json;
use uuid::Uuid;

use super::container_images;

#[derive(Debug)]
pub struct SidecarInjectionOptions<'a> {
    pub environment_id: Uuid,
    pub workload_id: Uuid,
    pub namespace: &'a str,
    pub auth_token: &'a str,
    pub manager_namespace: Option<&'a str>,
    pub proxy_routes: &'a [ProxyPortRoute],
}

/// Rebuild workload YAML so it reflects the latest container configuration while
/// stripping server-managed metadata. Mirrors kube-manager's clean_* helpers.
pub fn rebuild_workload_yaml(
    kind: &KubeWorkloadKind,
    yaml: &str,
    containers: &[KubeContainerInfo],
    sidecar: Option<&SidecarInjectionOptions<'_>>,
) -> Result<String> {
    match kind {
        KubeWorkloadKind::Deployment => {
            let deployment: Deployment = serde_yaml::from_str(yaml)?;
            let mut cleaned = clean_deployment(deployment, containers);
            if let Some(options) = sidecar {
                inject_sidecar_proxy_into_deployment(&mut cleaned, options)?;
            }
            Ok(serde_yaml::to_string(&cleaned)?)
        }
        KubeWorkloadKind::StatefulSet => {
            let statefulset: StatefulSet = serde_yaml::from_str(yaml)?;
            let cleaned = clean_statefulset(statefulset, containers);
            Ok(serde_yaml::to_string(&cleaned)?)
        }
        KubeWorkloadKind::DaemonSet => {
            let daemonset: DaemonSet = serde_yaml::from_str(yaml)?;
            let cleaned = clean_daemonset(daemonset, containers);
            Ok(serde_yaml::to_string(&cleaned)?)
        }
        KubeWorkloadKind::ReplicaSet => {
            let replicaset: ReplicaSet = serde_yaml::from_str(yaml)?;
            let cleaned = clean_replicaset(replicaset, containers);
            Ok(serde_yaml::to_string(&cleaned)?)
        }
        KubeWorkloadKind::Pod => {
            let pod: Pod = serde_yaml::from_str(yaml)?;
            let cleaned = clean_pod(pod, containers);
            Ok(serde_yaml::to_string(&cleaned)?)
        }
        KubeWorkloadKind::Job => {
            let job: Job = serde_yaml::from_str(yaml)?;
            let cleaned = clean_job(job, containers);
            Ok(serde_yaml::to_string(&cleaned)?)
        }
        KubeWorkloadKind::CronJob => {
            let cronjob: CronJob = serde_yaml::from_str(yaml)?;
            let cleaned = clean_cronjob(cronjob, containers);
            Ok(serde_yaml::to_string(&cleaned)?)
        }
    }
}

pub fn inject_sidecar_proxy_into_deployment(
    deployment: &mut Deployment,
    options: &SidecarInjectionOptions<'_>,
) -> Result<()> {
    let spec = match deployment.spec.as_mut() {
        Some(spec) => spec,
        None => return Err(anyhow!("Deployment spec missing for sidecar injection")),
    };

    let pod_spec = spec
        .template
        .spec
        .as_mut()
        .ok_or_else(|| anyhow!("Deployment pod spec missing for sidecar injection"))?;

    let routes_json = if options.proxy_routes.is_empty() {
        None
    } else {
        Some(serde_json::to_string(options.proxy_routes)?)
    };

    let sidecar_index = ensure_sidecar_proxy_container(
        pod_spec,
        options.environment_id,
        options.workload_id,
        options.namespace,
        options.auth_token,
        options.manager_namespace,
        routes_json.as_deref(),
    );

    if let (Some(json), Some(container)) = (routes_json, pod_spec.containers.get_mut(sidecar_index))
    {
        upsert_env_var(
            container,
            "LAPDEV_SIDECAR_PROXY_PORT_ROUTES",
            json.to_string(),
        );
    }

    Ok(())
}

#[allow(dead_code)]
pub fn clean_configmap(configmap: ConfigMap) -> ConfigMap {
    ConfigMap {
        metadata: clean_metadata(configmap.metadata),
        data: configmap.data,
        binary_data: configmap.binary_data,
        immutable: configmap.immutable,
    }
}

#[allow(dead_code)]
pub fn clean_secret(secret: Secret) -> Secret {
    Secret {
        metadata: clean_metadata(secret.metadata),
        data: secret.data,
        string_data: secret.string_data,
        type_: secret.type_,
        immutable: secret.immutable,
    }
}

#[allow(dead_code)]
pub fn clean_service(service: Service) -> Service {
    let clean_spec = service.spec.map(|mut original_spec| {
        original_spec.cluster_ip = None;
        original_spec.cluster_ips = None;
        original_spec.health_check_node_port = None;

        if let Some(ports) = original_spec.ports.as_mut() {
            for port in ports {
                port.node_port = None;
            }
        }

        original_spec
    });

    Service {
        metadata: clean_metadata(service.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_metadata(metadata: ObjectMeta) -> ObjectMeta {
    ObjectMeta {
        name: metadata.name,
        labels: metadata.labels,
        ..Default::default()
    }
}

fn clean_deployment(
    deployment: Deployment,
    workload_containers: &[KubeContainerInfo],
) -> Deployment {
    let clean_spec = deployment.spec.map(|original_spec| {
        let template = merge_template_containers(original_spec.template, workload_containers);

        DeploymentSpec {
            replicas: Some(1),
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
        metadata: clean_metadata(deployment.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_statefulset(
    statefulset: StatefulSet,
    workload_containers: &[KubeContainerInfo],
) -> StatefulSet {
    let clean_spec = statefulset.spec.map(|original_spec| {
        let template = merge_template_containers(original_spec.template, workload_containers);

        StatefulSetSpec {
            service_name: original_spec.service_name,
            replicas: Some(1),
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
        metadata: clean_metadata(statefulset.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_daemonset(daemonset: DaemonSet, workload_containers: &[KubeContainerInfo]) -> DaemonSet {
    let clean_spec = daemonset.spec.map(|original_spec| {
        let template = merge_template_containers(original_spec.template, workload_containers);

        DaemonSetSpec {
            selector: original_spec.selector,
            template,
            update_strategy: original_spec.update_strategy,
            min_ready_seconds: original_spec.min_ready_seconds,
            revision_history_limit: original_spec.revision_history_limit,
        }
    });

    DaemonSet {
        metadata: clean_metadata(daemonset.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_replicaset(
    replicaset: ReplicaSet,
    workload_containers: &[KubeContainerInfo],
) -> ReplicaSet {
    let clean_spec = replicaset.spec.map(|original_spec| {
        let template = original_spec
            .template
            .map(|t| merge_template_containers(t, workload_containers));

        ReplicaSetSpec {
            replicas: Some(1),
            selector: original_spec.selector,
            template,
            min_ready_seconds: original_spec.min_ready_seconds,
        }
    });

    ReplicaSet {
        metadata: clean_metadata(replicaset.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_pod(pod: Pod, workload_containers: &[KubeContainerInfo]) -> Pod {
    let clean_spec = pod.spec.map(|original_spec| {
        let merged_containers = merge_containers(original_spec.containers, workload_containers);

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
            service_account_name: None,
            service_account: None,
            automount_service_account_token: None,
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
            ..Default::default()
        }
    });

    Pod {
        metadata: clean_metadata(pod.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_job(job: Job, workload_containers: &[KubeContainerInfo]) -> Job {
    let clean_spec = job.spec.map(|original_spec| {
        let template = merge_template_containers(original_spec.template, workload_containers);

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
        metadata: clean_metadata(job.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn clean_cronjob(cronjob: CronJob, workload_containers: &[KubeContainerInfo]) -> CronJob {
    let clean_spec = cronjob.spec.map(|original_spec| {
        let mut job_template = original_spec.job_template;
        if let Some(job_spec) = &mut job_template.spec {
            job_spec.template =
                merge_template_containers(job_spec.template.clone(), workload_containers);
        }

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
        metadata: clean_metadata(cronjob.metadata),
        spec: clean_spec,
        status: None,
    }
}

fn merge_template_containers(
    template: PodTemplateSpec,
    workload_containers: &[KubeContainerInfo],
) -> PodTemplateSpec {
    let pod_spec = template.spec.map(|original_pod_spec| {
        let mut new_spec = original_pod_spec;
        let merged_containers = merge_containers(new_spec.containers, workload_containers);
        new_spec.containers = merged_containers;
        new_spec.service_account_name = None;
        new_spec.service_account = None;
        new_spec.automount_service_account_token = None;
        new_spec
    });

    PodTemplateSpec {
        spec: pod_spec,
        ..template
    }
}

fn merge_containers(
    containers: Vec<Container>,
    workload_containers: &[KubeContainerInfo],
) -> Vec<Container> {
    containers
        .into_iter()
        .map(|container| {
            if let Some(workload_container) = workload_containers
                .iter()
                .find(|wc| wc.name == container.name)
            {
                merge_single_container(container, workload_container)
            } else {
                container
            }
        })
        .collect()
}

fn merge_single_container(
    container: Container,
    workload_container: &KubeContainerInfo,
) -> Container {
    let mut new_container = container.clone();

    match &workload_container.image {
        KubeContainerImage::FollowOriginal => {}
        KubeContainerImage::Custom(custom_image) => {
            if !custom_image.is_empty() {
                new_container.image = Some(custom_image.clone());
            }
        }
    }

    let mut resources = container.resources.unwrap_or_default();
    let mut requests = resources.requests.unwrap_or_default();
    let mut limits = resources.limits.unwrap_or_default();

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

    let mut env_map: HashMap<String, (Option<String>, Option<EnvVarSource>)> = HashMap::new();

    if let Some(original_env) = container.env {
        for env_var in original_env {
            env_map.insert(env_var.name.clone(), (env_var.value, env_var.value_from));
        }
    }

    for kube_env_var in &workload_container.env_vars {
        env_map.insert(
            kube_env_var.name.clone(),
            (Some(kube_env_var.value.clone()), None),
        );
    }

    let merged_env: Vec<EnvVar> = env_map
        .into_iter()
        .map(|(name, (value, value_from))| EnvVar {
            name,
            value,
            value_from,
        })
        .collect();

    new_container.env = if merged_env.is_empty() {
        None
    } else {
        Some(merged_env)
    };

    if !workload_container.ports.is_empty() {
        let ports: Vec<ContainerPort> = workload_container
            .ports
            .iter()
            .map(|port| ContainerPort {
                name: port.name.clone(),
                container_port: port.container_port,
                protocol: port.protocol.clone(),
                ..Default::default()
            })
            .collect();
        new_container.ports = Some(ports);
    }

    new_container
}

fn ensure_sidecar_proxy_container(
    pod_spec: &mut PodSpec,
    environment_id: Uuid,
    workload_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_namespace: Option<&str>,
    proxy_routes_json: Option<&str>,
) -> usize {
    let manager_namespace = manager_namespace.unwrap_or("lapdev");
    let manager_addr = format!("lapdev-kube-manager.{manager_namespace}.svc:5001");

    if let Some(index) = pod_spec
        .containers
        .iter()
        .position(|container| container.name == "lapdev-sidecar-proxy")
    {
        let container = &mut pod_spec.containers[index];
        ensure_sidecar_ports(container);
        apply_sidecar_env(
            container,
            environment_id,
            workload_id,
            namespace,
            auth_token,
            &manager_addr,
        );
        if let Some(json) = proxy_routes_json {
            upsert_env_var(
                container,
                "LAPDEV_SIDECAR_PROXY_PORT_ROUTES",
                json.to_string(),
            );
        }
        return index;
    }

    let container = build_sidecar_proxy_container(
        environment_id,
        workload_id,
        namespace,
        auth_token,
        &manager_addr,
        proxy_routes_json.map(|value| value.to_string()),
    );
    pod_spec.containers.push(container);
    pod_spec.containers.len() - 1
}

fn build_sidecar_proxy_container(
    environment_id: Uuid,
    workload_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_addr: &str,
    proxy_routes_json: Option<String>,
) -> Container {
    let sidecar_image = container_images::sidecar_proxy_image_reference();

    let mut container = Container {
        name: "lapdev-sidecar-proxy".to_string(),
        image: Some(sidecar_image),
        image_pull_policy: Some("Always".to_string()),
        ..Default::default()
    };

    ensure_sidecar_ports(&mut container);
    apply_sidecar_env(
        &mut container,
        environment_id,
        workload_id,
        namespace,
        auth_token,
        manager_addr,
    );

    if let Some(routes_json) = proxy_routes_json {
        upsert_env_var(
            &mut container,
            "LAPDEV_SIDECAR_PROXY_PORT_ROUTES",
            routes_json,
        );
    }

    container
}

fn ensure_sidecar_ports(container: &mut Container) {
    let ports = container.ports.get_or_insert_with(Vec::new);

    if !ports
        .iter()
        .any(|port| port.container_port == DEFAULT_SIDECAR_PROXY_PORT as i32)
    {
        ports.push(ContainerPort {
            container_port: DEFAULT_SIDECAR_PROXY_PORT as i32,
            name: Some("proxy".to_string()),
            protocol: Some("TCP".to_string()),
            ..Default::default()
        });
    }

    if !ports
        .iter()
        .any(|port| port.container_port == DEFAULT_SIDECAR_PROXY_METRICS_PORT as i32)
    {
        ports.push(ContainerPort {
            container_port: DEFAULT_SIDECAR_PROXY_METRICS_PORT as i32,
            name: Some("metrics".to_string()),
            protocol: Some("TCP".to_string()),
            ..Default::default()
        });
    }
}

fn apply_sidecar_env(
    container: &mut Container,
    environment_id: Uuid,
    workload_id: Uuid,
    namespace: &str,
    auth_token: &str,
    manager_addr: &str,
) {
    upsert_env_var(
        container,
        "LAPDEV_ENVIRONMENT_ID",
        environment_id.to_string(),
    );
    upsert_env_var(
        container,
        "LAPDEV_ENVIRONMENT_AUTH_TOKEN",
        auth_token.to_string(),
    );
    upsert_env_var(container, "KUBERNETES_NAMESPACE", namespace.to_string());
    upsert_env_var(
        container,
        SIDECAR_PROXY_BIND_ADDR_ENV_VAR,
        DEFAULT_SIDECAR_PROXY_BIND_ADDR.to_string(),
    );
    upsert_env_var(
        container,
        SIDECAR_PROXY_PORT_ENV_VAR,
        DEFAULT_SIDECAR_PROXY_PORT.to_string(),
    );
    upsert_env_var(
        container,
        SIDECAR_PROXY_MANAGER_ADDR_ENV_VAR,
        manager_addr.to_string(),
    );
    upsert_env_var(
        container,
        SIDECAR_PROXY_WORKLOAD_ENV_VAR,
        workload_id.to_string(),
    );

    ensure_hostname_env(container);
}

fn ensure_hostname_env(container: &mut Container) {
    let env_list = container.env.get_or_insert_with(Vec::new);

    if env_list.iter().any(|var| var.name == "HOSTNAME") {
        return;
    }

    env_list.push(EnvVar {
        name: "HOSTNAME".to_string(),
        value: None,
        value_from: Some(EnvVarSource {
            field_ref: Some(ObjectFieldSelector {
                field_path: "metadata.name".to_string(),
                ..Default::default()
            }),
            ..Default::default()
        }),
        ..Default::default()
    });
}

fn upsert_env_var(container: &mut Container, name: &str, value: String) {
    let env_list = container.env.get_or_insert_with(Vec::new);

    if let Some(existing) = env_list.iter_mut().find(|var| var.name == name) {
        existing.value = Some(value);
        existing.value_from = None;
    } else {
        env_list.push(EnvVar {
            name: name.to_string(),
            value: Some(value),
            value_from: None,
            ..Default::default()
        });
    }
}

pub fn set_workload_replicas(workload: &mut KubeWorkloadYamlOnly, replicas: i32) -> Result<()> {
    match workload {
        KubeWorkloadYamlOnly::Deployment(yaml) => {
            let mut deployment: Deployment = serde_yaml::from_str(yaml)?;
            if let Some(spec) = deployment.spec.as_mut() {
                spec.replicas = Some(replicas);
            }
            *yaml = serde_yaml::to_string(&deployment)?;
        }
        KubeWorkloadYamlOnly::StatefulSet(yaml) => {
            let mut statefulset: StatefulSet = serde_yaml::from_str(yaml)?;
            if let Some(spec) = statefulset.spec.as_mut() {
                spec.replicas = Some(replicas);
            }
            *yaml = serde_yaml::to_string(&statefulset)?;
        }
        KubeWorkloadYamlOnly::ReplicaSet(yaml) => {
            let mut replicaset: ReplicaSet = serde_yaml::from_str(yaml)?;
            if let Some(spec) = replicaset.spec.as_mut() {
                spec.replicas = Some(replicas);
            }
            *yaml = serde_yaml::to_string(&replicaset)?;
        }
        _ => {}
    }
    Ok(())
}

pub fn set_cronjob_suspend(workload: &mut KubeWorkloadYamlOnly, suspend: bool) -> Result<()> {
    if let KubeWorkloadYamlOnly::CronJob(yaml) = workload {
        let mut cronjob: CronJob = serde_yaml::from_str(yaml)?;
        if let Some(spec) = cronjob.spec.as_mut() {
            spec.suspend = Some(suspend);
        }
        *yaml = serde_yaml::to_string(&cronjob)?;
    }

    Ok(())
}

pub fn set_daemonset_paused(workload: &mut KubeWorkloadYamlOnly, paused: bool) -> Result<()> {
    if let KubeWorkloadYamlOnly::DaemonSet(yaml) = workload {
        let mut daemonset: DaemonSet = serde_yaml::from_str(yaml)?;
        if let Some(spec) = daemonset.spec.as_mut() {
            if let Some(template) = spec.template.spec.as_mut() {
                let selector = template.node_selector.get_or_insert_with(Default::default);

                if paused {
                    selector.insert("lapdev.io/paused".to_string(), "true".to_string());
                } else if let Some(selector) = template.node_selector.as_mut() {
                    selector.remove("lapdev.io/paused");
                    if selector.is_empty() {
                        template.node_selector = None;
                    }
                }
            }
        }
        *yaml = serde_yaml::to_string(&daemonset)?;
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_yaml::Value;

    fn base_container(name: &str) -> KubeContainerInfo {
        KubeContainerInfo {
            name: name.to_string(),
            original_image: "example:image".to_string(),
            image: KubeContainerImage::FollowOriginal,
            cpu_request: None,
            cpu_limit: None,
            memory_request: None,
            memory_limit: None,
            env_vars: Vec::new(),
            original_env_vars: Vec::new(),
            ports: Vec::new(),
        }
    }

    fn assert_replica_clamped(kind: KubeWorkloadKind, yaml: &str) {
        let containers = vec![base_container("app")];
        let rebuilt = rebuild_workload_yaml(&kind, yaml, &containers, None).unwrap();
        let parsed: Value = serde_yaml::from_str(&rebuilt).unwrap();
        let replicas = parsed
            .get("spec")
            .and_then(|spec| spec.get("replicas"))
            .and_then(|value| value.as_i64());
        assert_eq!(
            replicas,
            Some(1),
            "replica count not clamped for {:?}",
            kind
        );
    }

    #[test]
    fn rebuild_workload_yaml_clamps_replicas_for_supported_kinds() {
        let deployment = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: test-deploy
spec:
  replicas: 3
  selector:
    matchLabels:
      app: app
  template:
    metadata:
      labels:
        app: app
    spec:
      containers:
        - name: app
          image: nginx
"#;
        assert_replica_clamped(KubeWorkloadKind::Deployment, deployment);

        let statefulset = r#"
apiVersion: apps/v1
kind: StatefulSet
metadata:
  name: test-sts
spec:
  serviceName: svc
  replicas: 4
  selector:
    matchLabels:
      app: app
  template:
    metadata:
      labels:
        app: app
    spec:
      containers:
        - name: app
          image: nginx
"#;
        assert_replica_clamped(KubeWorkloadKind::StatefulSet, statefulset);

        let replicaset = r#"
apiVersion: apps/v1
kind: ReplicaSet
metadata:
  name: test-rs
spec:
  replicas: 5
  selector:
    matchLabels:
      app: app
  template:
    metadata:
      labels:
        app: app
    spec:
      containers:
        - name: app
          image: nginx
"#;
        assert_replica_clamped(KubeWorkloadKind::ReplicaSet, replicaset);
    }
}
