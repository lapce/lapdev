use std::collections::{BTreeMap, HashSet};

use anyhow::anyhow;
use k8s_openapi::api::core::v1::PodSpec;
use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::Pod,
};
use lapdev_common::kube::{
    KubeContainerImage, KubeContainerInfo, KubeContainerPort, KubeServicePort, KubeWorkloadDetails,
    KubeWorkloadKind,
};
use lapdev_db::api::CachedClusterService;
use lapdev_kube_rpc::KubeRawWorkloadYaml;

pub fn build_workload_details_from_yaml(
    raw: KubeRawWorkloadYaml,
    services: &[CachedClusterService],
) -> anyhow::Result<KubeWorkloadDetails> {
    let KubeRawWorkloadYaml {
        name,
        namespace,
        kind,
        workload_yaml,
    } = raw;

    let (containers, labels) = extract_containers_and_labels(&kind, &workload_yaml)?;
    let ports = ports_from_cached_services(&labels, services);

    Ok(KubeWorkloadDetails {
        name,
        namespace,
        kind,
        containers,
        ports,
        workload_yaml,
        base_workload_id: None,
    })
}

fn extract_containers_and_labels(
    kind: &KubeWorkloadKind,
    workload_yaml: &str,
) -> anyhow::Result<(Vec<KubeContainerInfo>, BTreeMap<String, String>)> {
    match kind {
        KubeWorkloadKind::Deployment => {
            let deployment: Deployment = serde_yaml::from_str(workload_yaml)?;
            let labels = deployment
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let pod_spec = deployment
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .ok_or_else(|| anyhow!("Deployment missing pod spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
        KubeWorkloadKind::StatefulSet => {
            let statefulset: StatefulSet = serde_yaml::from_str(workload_yaml)?;
            let labels = statefulset
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let pod_spec = statefulset
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .ok_or_else(|| anyhow!("StatefulSet missing pod spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
        KubeWorkloadKind::DaemonSet => {
            let daemonset: DaemonSet = serde_yaml::from_str(workload_yaml)?;
            let labels = daemonset
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let pod_spec = daemonset
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .ok_or_else(|| anyhow!("DaemonSet missing pod spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
        KubeWorkloadKind::ReplicaSet => {
            let replicaset: ReplicaSet = serde_yaml::from_str(workload_yaml)?;
            let labels = replicaset
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let pod_spec = replicaset
                .spec
                .as_ref()
                .and_then(|s| s.template.as_ref())
                .and_then(|t| t.spec.as_ref())
                .ok_or_else(|| anyhow!("ReplicaSet missing pod spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
        KubeWorkloadKind::Pod => {
            let pod: Pod = serde_yaml::from_str(workload_yaml)?;
            let labels = pod.metadata.labels.clone().unwrap_or_default();
            let pod_spec = pod
                .spec
                .as_ref()
                .ok_or_else(|| anyhow!("Pod missing spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
        KubeWorkloadKind::Job => {
            let job: Job = serde_yaml::from_str(workload_yaml)?;
            let labels = job
                .spec
                .as_ref()
                .and_then(|s| s.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let pod_spec = job
                .spec
                .as_ref()
                .and_then(|s| s.template.spec.as_ref())
                .ok_or_else(|| anyhow!("Job missing pod spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
        KubeWorkloadKind::CronJob => {
            let cronjob: CronJob = serde_yaml::from_str(workload_yaml)?;
            let labels = cronjob
                .spec
                .as_ref()
                .and_then(|s| s.job_template.spec.as_ref())
                .and_then(|js| js.template.metadata.as_ref())
                .and_then(|m| m.labels.clone())
                .unwrap_or_default();
            let pod_spec = cronjob
                .spec
                .as_ref()
                .and_then(|s| s.job_template.spec.as_ref())
                .and_then(|js| js.template.spec.as_ref())
                .ok_or_else(|| anyhow!("CronJob missing pod spec"))?;
            let containers = extract_pod_spec_containers(pod_spec)?;
            Ok((containers, labels))
        }
    }
}

fn extract_pod_spec_containers(pod_spec: &PodSpec) -> anyhow::Result<Vec<KubeContainerInfo>> {
    pod_spec
        .containers
        .iter()
        .map(|container| {
            let mut cpu_request = None;
            let mut cpu_limit = None;
            let mut memory_request = None;
            let mut memory_limit = None;

            if let Some(resources) = &container.resources {
                if let Some(requests) = &resources.requests {
                    if let Some(cpu_req) = requests.get("cpu") {
                        cpu_request = Some(cpu_req.0.clone());
                    }
                    if let Some(memory_req) = requests.get("memory") {
                        memory_request = Some(memory_req.0.clone());
                    }
                }

                if let Some(limits) = &resources.limits {
                    if let Some(cpu_lim) = limits.get("cpu") {
                        cpu_limit = Some(cpu_lim.0.clone());
                    }
                    if let Some(memory_lim) = limits.get("memory") {
                        memory_limit = Some(memory_lim.0.clone());
                    }
                }
            }

            let image = container
                .image
                .clone()
                .ok_or_else(|| anyhow!("Container '{}' has no image specified", container.name))?;

            let ports = container
                .ports
                .as_ref()
                .map(|ports| {
                    ports
                        .iter()
                        .map(|port| KubeContainerPort {
                            name: port.name.clone(),
                            container_port: port.container_port,
                            protocol: port.protocol.clone(),
                        })
                        .collect::<Vec<_>>()
                })
                .unwrap_or_default();

            Ok(KubeContainerInfo {
                name: container.name.clone(),
                original_image: image.clone(),
                image: KubeContainerImage::FollowOriginal,
                cpu_request,
                cpu_limit,
                memory_request,
                memory_limit,
                env_vars: Vec::new(),
                original_env_vars: Vec::new(),
                ports,
            })
        })
        .collect()
}

fn ports_from_cached_services(
    workload_labels: &BTreeMap<String, String>,
    services: &[CachedClusterService],
) -> Vec<KubeServicePort> {
    let mut seen = HashSet::new();
    let mut ports = Vec::new();

    for service in services {
        if service.selector.is_empty() {
            continue;
        }

        let matches = service
            .selector
            .iter()
            .all(|(key, value)| workload_labels.get(key).map_or(false, |v| v == value));

        if !matches {
            continue;
        }

        for port in &service.ports {
            let original_target = port.original_target_port.or(port.target_port);
            let key = (port.port, original_target, port.protocol.clone());
            if seen.insert(key) {
                ports.push(port.clone());
            }
        }
    }

    ports
}
