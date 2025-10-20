use uuid::Uuid;

use std::{
    collections::{BTreeMap, HashMap},
    str::FromStr,
};

use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{Pod, Service},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use lapdev_common::kube::{KubeServiceDetails, KubeServiceWithYaml, KubeWorkloadKind};
use lapdev_kube_rpc::{KubeWorkloadYamlOnly, KubeWorkloadsWithResources};
use lapdev_rpc::error::ApiError;

use super::{resources::rebuild_workload_yaml, KubeController};

impl KubeController {
    pub async fn get_environment_workloads(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
    ) -> Result<Vec<lapdev_common::kube::KubeEnvironmentWorkload>, ApiError> {
        // Verify environment belongs to the organization
        let environment = self
            .db
            .get_kube_environment(environment_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;

        // Check authorization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // If it's a personal environment, check ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        self.db
            .get_environment_workloads(environment_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn get_environment_workload(
        &self,
        org_id: Uuid,
        workload_id: Uuid,
    ) -> Result<Option<lapdev_common::kube::KubeEnvironmentWorkload>, ApiError> {
        // First get the workload to find its environment
        if let Some(workload) = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)?
        {
            // Verify the environment belongs to the organization
            let environment = self
                .db
                .get_kube_environment(workload.environment_id)
                .await
                .map_err(ApiError::from)?
                .ok_or_else(|| ApiError::InvalidRequest("Environment not found".to_string()))?;
            if environment.organization_id != org_id {
                return Err(ApiError::Unauthorized);
            }
            Ok(Some(workload))
        } else {
            Ok(None)
        }
    }

    pub async fn delete_environment_workload(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        workload_id: Uuid,
        environment: lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), ApiError> {
        // Verify the environment belongs to the organization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // For personal/branch environments, verify ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        // Delete the workload
        self.db
            .delete_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)
    }

    pub async fn update_environment_workload(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        workload_id: Uuid,
        containers: Vec<lapdev_common::kube::KubeContainerInfo>,
        environment: lapdev_db_entities::kube_environment::Model,
    ) -> Result<(), ApiError> {
        // Verify the environment belongs to the organization
        if environment.organization_id != org_id {
            return Err(ApiError::Unauthorized);
        }

        // For personal/branch environments, verify ownership
        if !environment.is_shared && environment.user_id != user_id {
            return Err(ApiError::Unauthorized);
        }

        let existing_workload = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Workload not found".to_string()))?;

        let kind = KubeWorkloadKind::from_str(&existing_workload.kind).map_err(|_| {
            ApiError::InvalidRequest(format!(
                "Invalid workload kind {} for environment {}",
                existing_workload.kind, existing_workload.environment_id
            ))
        })?;

        let (workloads_with_resources, persisted_yaml, extra_labels) =
            if environment.base_environment_id.is_some() {
                let (manifest, yaml, labels) = self
                    .build_branch_workload_manifest(
                        &environment,
                        &existing_workload,
                        kind.clone(),
                        &containers,
                    )
                    .await?;
                (manifest, yaml, Some(labels))
            } else {
                let (manifest, yaml) = Self::build_standard_workload_manifest(
                    kind,
                    &existing_workload.workload_yaml,
                    &containers,
                )?;
                (manifest, yaml, None)
            };

        let cluster_server = self
            .get_random_kube_cluster_server(environment.cluster_id)
            .await
            .ok_or_else(|| {
                ApiError::InvalidRequest("No connected KubeManager for this cluster".to_string())
            })?;

        self.deploy_environment_resources(
            &cluster_server,
            &environment,
            workloads_with_resources,
            extra_labels,
        )
        .await?;

        if environment.base_environment_id.is_some() {
            if let Err(err) = cluster_server
                .rpc_client
                .refresh_branch_service_routes(tarpc::context::current(), environment.id)
                .await
            {
                tracing::warn!(
                    "Failed to refresh branch service routes for environment {}: {}",
                    environment.id,
                    err
                );
            }
        }

        self.db
            .update_environment_workload(workload_id, containers, persisted_yaml)
            .await
            .map_err(ApiError::from)?;

        Ok(())
    }

    async fn build_branch_workload_manifest(
        &self,
        environment: &lapdev_db_entities::kube_environment::Model,
        existing_workload: &lapdev_common::kube::KubeEnvironmentWorkload,
        kind: KubeWorkloadKind,
        containers: &[lapdev_common::kube::KubeContainerInfo],
    ) -> Result<(KubeWorkloadsWithResources, String, HashMap<String, String>), ApiError> {
        let base_workload_name = existing_workload.name.clone();
        let env_suffix = environment.id.to_string();
        let branch_workload_name = if base_workload_name.ends_with(&env_suffix) {
            base_workload_name.clone()
        } else {
            format!("{}-{}", base_workload_name, environment.id)
        };

        let (mut workloads_with_resources, _) = Self::build_standard_workload_manifest(
            kind,
            &existing_workload.workload_yaml,
            containers,
        )?;

        for workload_yaml in &mut workloads_with_resources.workloads {
            rename_workload_yaml(workload_yaml, &base_workload_name, &branch_workload_name)?;
        }

        let environment_services = self
            .db
            .get_environment_services(environment.id)
            .await
            .map_err(ApiError::from)?;

        let mut updated_services: HashMap<String, KubeServiceWithYaml> = HashMap::new();
        for service in environment_services {
            let mut matches_workload =
                service.name == base_workload_name || service.name == branch_workload_name;
            if !matches_workload {
                matches_workload = service
                    .selector
                    .values()
                    .any(|value| value == &base_workload_name || value == &branch_workload_name);
            }
            if !matches_workload {
                continue;
            }

            if service.yaml.trim().is_empty() {
                tracing::warn!(
                    environment_id = %environment.id,
                    workload = %existing_workload.name,
                    service = %service.name,
                    "Branch workload service YAML is empty; skipping service deployment"
                );
                continue;
            }

            let branch_service_name = if service.name.ends_with(&env_suffix) {
                service.name.clone()
            } else {
                format!("{}-{}", service.name, environment.id)
            };
            let renamed_yaml = rename_service_yaml(
                &service.yaml,
                &service.name,
                &branch_service_name,
                &base_workload_name,
                &branch_workload_name,
            )?;

            let mut selector = service.selector.clone();
            for value in selector.values_mut() {
                if value == &base_workload_name || value == &branch_workload_name {
                    *value = branch_workload_name.clone();
                }
            }
            selector
                .entry("app".to_string())
                .or_insert_with(|| branch_workload_name.clone());
            selector
                .entry("lapdev.workload".to_string())
                .or_insert_with(|| branch_workload_name.clone());

            updated_services.insert(
                branch_service_name.clone(),
                KubeServiceWithYaml {
                    yaml: renamed_yaml,
                    details: KubeServiceDetails {
                        name: branch_service_name,
                        ports: service.ports.clone(),
                        selector,
                    },
                },
            );
        }
        workloads_with_resources.services = updated_services;
        workloads_with_resources.configmaps.clear();
        workloads_with_resources.secrets.clear();

        let persisted_yaml = extract_workload_yaml(&workloads_with_resources.workloads)?;

        let mut extra_labels = HashMap::new();
        extra_labels.insert(
            "lapdev.branch-environment".to_string(),
            environment.id.to_string(),
        );
        extra_labels.insert(
            "lapdev.base-workload".to_string(),
            existing_workload.name.clone(),
        );
        extra_labels.insert(
            "lapdev.io/routing-key".to_string(),
            format!("branch-{}", environment.id),
        );
        extra_labels.insert(
            "lapdev.io/proxy-target-port".to_string(),
            "8080".to_string(),
        );

        Ok((workloads_with_resources, persisted_yaml, extra_labels))
    }

    fn build_standard_workload_manifest(
        kind: KubeWorkloadKind,
        original_yaml: &str,
        containers: &[lapdev_common::kube::KubeContainerInfo],
    ) -> Result<(KubeWorkloadsWithResources, String), ApiError> {
        let rebuilt_yaml =
            rebuild_workload_yaml(&kind, original_yaml, containers).map_err(|err| {
                ApiError::InvalidRequest(format!("Failed to rebuild workload manifest: {}", err))
            })?;

        let workload = match kind {
            KubeWorkloadKind::Deployment => KubeWorkloadYamlOnly::Deployment(rebuilt_yaml.clone()),
            KubeWorkloadKind::StatefulSet => {
                KubeWorkloadYamlOnly::StatefulSet(rebuilt_yaml.clone())
            }
            KubeWorkloadKind::DaemonSet => KubeWorkloadYamlOnly::DaemonSet(rebuilt_yaml.clone()),
            KubeWorkloadKind::ReplicaSet => KubeWorkloadYamlOnly::ReplicaSet(rebuilt_yaml.clone()),
            KubeWorkloadKind::Pod => KubeWorkloadYamlOnly::Pod(rebuilt_yaml.clone()),
            KubeWorkloadKind::Job => KubeWorkloadYamlOnly::Job(rebuilt_yaml.clone()),
            KubeWorkloadKind::CronJob => KubeWorkloadYamlOnly::CronJob(rebuilt_yaml.clone()),
        };

        Ok((
            KubeWorkloadsWithResources {
                workloads: vec![workload],
                services: HashMap::new(),
                configmaps: HashMap::new(),
                secrets: HashMap::new(),
            },
            rebuilt_yaml,
        ))
    }
}

fn extract_workload_yaml(workloads: &[KubeWorkloadYamlOnly]) -> Result<String, ApiError> {
    let workload = workloads.first().ok_or_else(|| {
        ApiError::InvalidRequest("No workload manifest generated for deployment".to_string())
    })?;

    let yaml = match workload {
        KubeWorkloadYamlOnly::Deployment(yaml)
        | KubeWorkloadYamlOnly::StatefulSet(yaml)
        | KubeWorkloadYamlOnly::DaemonSet(yaml)
        | KubeWorkloadYamlOnly::ReplicaSet(yaml)
        | KubeWorkloadYamlOnly::Pod(yaml)
        | KubeWorkloadYamlOnly::Job(yaml)
        | KubeWorkloadYamlOnly::CronJob(yaml) => yaml.clone(),
    };

    Ok(yaml)
}

fn rename_workload_yaml(
    workload: &mut KubeWorkloadYamlOnly,
    old_name: &str,
    new_name: &str,
) -> Result<(), ApiError> {
    match workload {
        KubeWorkloadYamlOnly::Deployment(yaml) => rename_deployment_yaml(yaml, old_name, new_name),
        KubeWorkloadYamlOnly::StatefulSet(yaml) => {
            rename_statefulset_yaml(yaml, old_name, new_name)
        }
        KubeWorkloadYamlOnly::DaemonSet(yaml) => rename_daemonset_yaml(yaml, old_name, new_name),
        KubeWorkloadYamlOnly::ReplicaSet(yaml) => rename_replicaset_yaml(yaml, old_name, new_name),
        KubeWorkloadYamlOnly::Pod(yaml) => rename_pod_yaml(yaml, old_name, new_name),
        KubeWorkloadYamlOnly::Job(yaml) => rename_job_yaml(yaml, old_name, new_name),
        KubeWorkloadYamlOnly::CronJob(yaml) => rename_cronjob_yaml(yaml, old_name, new_name),
    }
}

fn rename_service_yaml(
    yaml: &str,
    old_service_name: &str,
    new_service_name: &str,
    old_selector_value: &str,
    new_selector_value: &str,
) -> Result<String, ApiError> {
    let mut service: Service = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse service YAML for rename: {}", err))
    })?;

    update_object_meta(&mut service.metadata, old_service_name, new_service_name);
    ensure_label(&mut service.metadata.labels, "app", new_service_name);

    if let Some(spec) = service.spec.as_mut() {
        if let Some(selector) = spec.selector.as_mut() {
            for value in selector.values_mut() {
                if value == old_selector_value || value == old_service_name {
                    *value = new_selector_value.to_string();
                }
            }
            selector
                .entry("app".to_string())
                .or_insert_with(|| new_selector_value.to_string());
            selector
                .entry("lapdev.workload".to_string())
                .or_insert_with(|| new_selector_value.to_string());
        } else {
            let mut selector = BTreeMap::new();
            selector.insert("app".to_string(), new_selector_value.to_string());
            selector.insert(
                "lapdev.workload".to_string(),
                new_selector_value.to_string(),
            );
            spec.selector = Some(selector);
        }
    }

    serde_yaml::to_string(&service).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed service YAML: {}", err))
    })
}

fn rename_deployment_yaml(
    yaml: &mut String,
    old_name: &str,
    new_name: &str,
) -> Result<(), ApiError> {
    let mut deployment: Deployment = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse deployment YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut deployment.metadata, old_name, new_name);
    ensure_label(&mut deployment.metadata.labels, "app", new_name);
    ensure_label(&mut deployment.metadata.labels, "lapdev.workload", new_name);

    if let Some(spec) = deployment.spec.as_mut() {
        update_selector_labels(&mut spec.selector, old_name, new_name);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, old_name, new_name);
            ensure_label(&mut template.labels, "app", new_name);
            ensure_label(&mut template.labels, "lapdev.workload", new_name);
        } else {
            let mut metadata = ObjectMeta::default();
            ensure_label(&mut metadata.labels, "app", new_name);
            ensure_label(&mut metadata.labels, "lapdev.workload", new_name);
            spec.template.metadata = Some(metadata);
        }
    }

    *yaml = serde_yaml::to_string(&deployment).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize renamed deployment YAML: {}",
            err
        ))
    })?;
    Ok(())
}

fn rename_statefulset_yaml(
    yaml: &mut String,
    old_name: &str,
    new_name: &str,
) -> Result<(), ApiError> {
    let mut statefulset: StatefulSet = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse statefulset YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut statefulset.metadata, old_name, new_name);
    ensure_label(&mut statefulset.metadata.labels, "app", new_name);
    ensure_label(
        &mut statefulset.metadata.labels,
        "lapdev.workload",
        new_name,
    );

    if let Some(spec) = statefulset.spec.as_mut() {
        update_selector_labels(&mut spec.selector, old_name, new_name);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, old_name, new_name);
            ensure_label(&mut template.labels, "app", new_name);
            ensure_label(&mut template.labels, "lapdev.workload", new_name);
        }
    }

    *yaml = serde_yaml::to_string(&statefulset).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize renamed statefulset YAML: {}",
            err
        ))
    })?;
    Ok(())
}

fn rename_daemonset_yaml(
    yaml: &mut String,
    old_name: &str,
    new_name: &str,
) -> Result<(), ApiError> {
    let mut daemonset: DaemonSet = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse daemonset YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut daemonset.metadata, old_name, new_name);
    ensure_label(&mut daemonset.metadata.labels, "app", new_name);
    ensure_label(&mut daemonset.metadata.labels, "lapdev.workload", new_name);

    if let Some(spec) = daemonset.spec.as_mut() {
        update_selector_labels(&mut spec.selector, old_name, new_name);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, old_name, new_name);
            ensure_label(&mut template.labels, "app", new_name);
            ensure_label(&mut template.labels, "lapdev.workload", new_name);
        }
    }

    *yaml = serde_yaml::to_string(&daemonset).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize renamed daemonset YAML: {}",
            err
        ))
    })?;
    Ok(())
}

fn rename_replicaset_yaml(
    yaml: &mut String,
    old_name: &str,
    new_name: &str,
) -> Result<(), ApiError> {
    let mut replicaset: ReplicaSet = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse replicaset YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut replicaset.metadata, old_name, new_name);
    ensure_label(&mut replicaset.metadata.labels, "app", new_name);
    ensure_label(&mut replicaset.metadata.labels, "lapdev.workload", new_name);

    if let Some(spec) = replicaset.spec.as_mut() {
        update_selector_labels(&mut spec.selector, old_name, new_name);
        if let Some(template) = spec.template.as_mut() {
            if let Some(metadata) = template.metadata.as_mut() {
                update_object_meta(metadata, old_name, new_name);
                ensure_label(&mut metadata.labels, "app", new_name);
                ensure_label(&mut metadata.labels, "lapdev.workload", new_name);
            }
        }
    }

    *yaml = serde_yaml::to_string(&replicaset).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to serialize renamed replicaset YAML: {}",
            err
        ))
    })?;
    Ok(())
}

fn rename_pod_yaml(yaml: &mut String, old_name: &str, new_name: &str) -> Result<(), ApiError> {
    let mut pod: Pod = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse pod YAML for rename: {}", err))
    })?;

    update_object_meta(&mut pod.metadata, old_name, new_name);
    ensure_label(&mut pod.metadata.labels, "app", new_name);
    ensure_label(&mut pod.metadata.labels, "lapdev.workload", new_name);

    *yaml = serde_yaml::to_string(&pod).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed pod YAML: {}", err))
    })?;
    Ok(())
}

fn rename_job_yaml(yaml: &mut String, old_name: &str, new_name: &str) -> Result<(), ApiError> {
    let mut job: Job = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse job YAML for rename: {}", err))
    })?;

    update_object_meta(&mut job.metadata, old_name, new_name);
    ensure_label(&mut job.metadata.labels, "app", new_name);
    ensure_label(&mut job.metadata.labels, "lapdev.workload", new_name);

    if let Some(spec) = job.spec.as_mut() {
        update_optional_selector_labels(&mut spec.selector, old_name, new_name);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, old_name, new_name);
            ensure_label(&mut template.labels, "app", new_name);
            ensure_label(&mut template.labels, "lapdev.workload", new_name);
        }
    }

    *yaml = serde_yaml::to_string(&job).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed job YAML: {}", err))
    })?;
    Ok(())
}

fn rename_cronjob_yaml(yaml: &mut String, old_name: &str, new_name: &str) -> Result<(), ApiError> {
    let mut cronjob: CronJob = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse cronjob YAML for rename: {}", err))
    })?;

    update_object_meta(&mut cronjob.metadata, old_name, new_name);
    ensure_label(&mut cronjob.metadata.labels, "app", new_name);
    ensure_label(&mut cronjob.metadata.labels, "lapdev.workload", new_name);

    if let Some(spec) = cronjob.spec.as_mut() {
        let job_template = &mut spec.job_template;
        if let Some(job_spec) = job_template.spec.as_mut() {
            update_optional_selector_labels(&mut job_spec.selector, old_name, new_name);
            if let Some(template) = job_spec.template.metadata.as_mut() {
                update_object_meta(template, old_name, new_name);
                ensure_label(&mut template.labels, "app", new_name);
                ensure_label(&mut template.labels, "lapdev.workload", new_name);
            }
        }
    }

    *yaml = serde_yaml::to_string(&cronjob).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed cronjob YAML: {}", err))
    })?;
    Ok(())
}

fn update_object_meta(metadata: &mut ObjectMeta, old_value: &str, new_value: &str) {
    metadata.name = Some(new_value.to_string());
    if let Some(labels) = metadata.labels.as_mut() {
        for value in labels.values_mut() {
            if value == old_value {
                *value = new_value.to_string();
            }
        }
    }
}

fn ensure_label(labels: &mut Option<BTreeMap<String, String>>, key: &str, value: &str) {
    let map = labels.get_or_insert_with(BTreeMap::new);
    map.insert(key.to_string(), value.to_string());
}

fn update_selector_labels(selector: &mut LabelSelector, old_value: &str, new_value: &str) {
    if let Some(match_labels) = selector.match_labels.as_mut() {
        for value in match_labels.values_mut() {
            if value == old_value {
                *value = new_value.to_string();
            }
        }
        match_labels
            .entry("app".to_string())
            .or_insert_with(|| new_value.to_string());
        match_labels
            .entry("lapdev.workload".to_string())
            .or_insert_with(|| new_value.to_string());
    } else {
        let mut map = BTreeMap::new();
        map.insert("app".to_string(), new_value.to_string());
        map.insert("lapdev.workload".to_string(), new_value.to_string());
        selector.match_labels = Some(map);
    }
}

fn update_optional_selector_labels(
    selector: &mut Option<LabelSelector>,
    old_value: &str,
    new_value: &str,
) {
    match selector {
        Some(existing) => update_selector_labels(existing, old_value, new_value),
        None => {
            let mut new_selector = LabelSelector::default();
            update_selector_labels(&mut new_selector, old_value, new_value);
            *selector = Some(new_selector);
        }
    }
}
