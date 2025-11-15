use uuid::Uuid;

use std::{
    collections::{BTreeMap, HashMap, HashSet},
    str::FromStr,
};

use k8s_openapi::api::{
    apps::v1::{DaemonSet, Deployment, ReplicaSet, StatefulSet},
    batch::v1::{CronJob, Job},
    core::v1::{Pod, Service},
};
use k8s_openapi::apimachinery::pkg::apis::meta::v1::{LabelSelector, ObjectMeta};
use lapdev_common::kube::{
    KubeEnvironment, KubeEnvironmentWorkload, KubeEnvironmentWorkloadDetail, KubeServiceDetails,
    KubeServiceWithYaml, KubeWorkloadKind,
};
use lapdev_kube::server::KubeClusterServer;
use lapdev_kube_rpc::{
    KubeWorkloadWithResources, KubeWorkloadYamlOnly, KubeWorkloadsWithResources,
    ProxyBranchRouteConfig,
};
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

        let mut workloads = self
            .db
            .get_environment_workloads(environment_id)
            .await
            .map_err(ApiError::from)?;

        if let Some(base_environment_id) = environment.base_environment_id {
            let base_workloads = self
                .db
                .get_environment_workloads(base_environment_id)
                .await
                .map_err(ApiError::from)?;
            let overridden: HashSet<Uuid> = workloads
                .iter()
                .filter_map(|w| w.base_workload_id)
                .collect();

            for base_workload in base_workloads {
                if overridden.contains(&base_workload.id) {
                    continue;
                }
                let mut inherited = base_workload.clone();
                inherited.environment_id = environment.id;
                if inherited.base_workload_id.is_none() {
                    inherited.base_workload_id = Some(base_workload.id);
                }
                workloads.push(inherited);
            }
        }

        Ok(workloads)
    }

    pub async fn get_environment_workload_detail(
        &self,
        org_id: Uuid,
        user_id: Uuid,
        environment_id: Uuid,
        workload_id: Uuid,
    ) -> Result<KubeEnvironmentWorkloadDetail, ApiError> {
        let environment = self
            .get_kube_environment(org_id, user_id, environment_id)
            .await?;

        let workload = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Workload not found".to_string()))?;

        let workload = Self::map_workload_to_environment(&environment, workload)?;

        Ok(KubeEnvironmentWorkloadDetail {
            environment,
            workload,
        })
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
    ) -> Result<Uuid, ApiError> {
        let existing_workload = self
            .db
            .get_environment_workload(workload_id)
            .await
            .map_err(ApiError::from)?
            .ok_or_else(|| ApiError::InvalidRequest("Workload not found".to_string()))?;

        let mut workload_record = existing_workload;
        if let Some(base_environment_id) = environment.base_environment_id {
            if workload_record.environment_id == base_environment_id {
                workload_record = self
                    .ensure_branch_workload_override(&environment, &workload_record)
                    .await?;
            }
        }

        let kind = KubeWorkloadKind::from_str(&workload_record.kind).map_err(|_| {
            ApiError::InvalidRequest(format!(
                "Invalid workload kind {} for environment {}",
                workload_record.kind, workload_record.environment_id
            ))
        })?;

        let target_workload_id = workload_record.id;

        let (workload_with_resources, persisted_yaml, extra_labels) =
            if environment.base_environment_id.is_some() {
                let (manifest, yaml, labels) = self
                    .build_branch_workload_manifest(
                        &environment,
                        &workload_record,
                        kind.clone(),
                        &containers,
                    )
                    .await?;
                (manifest, yaml, Some(labels))
            } else {
                let (manifest, yaml) = Self::build_standard_workload_manifest(
                    kind,
                    &workload_record.workload_yaml,
                    &containers,
                )?;
                let mut labels = HashMap::new();
                labels.insert(
                    "lapdev.base-workload".to_string(),
                    workload_record.name.clone(),
                );
                labels.insert(
                    "lapdev.base-workload-id".to_string(),
                    workload_record.id.to_string(),
                );
                (manifest, yaml, Some(labels))
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
            KubeWorkloadsWithResources {
                workloads: vec![workload_with_resources.workload],
                services: workload_with_resources.services,
                configmaps: workload_with_resources.configmaps,
                secrets: workload_with_resources.secrets,
            },
            extra_labels,
        )
        .await?;

        if let Some(base_environment_id) = environment.base_environment_id {
            match workload_record.base_workload_id {
                Some(base_workload_id) => {
                    let base_environment = self
                        .db
                        .get_kube_environment(base_environment_id)
                        .await
                        .map_err(ApiError::from)?
                        .ok_or_else(|| {
                            ApiError::InvalidRequest(format!(
                                "Base environment {} not found",
                                base_environment_id
                            ))
                        })?;

                    match cluster_server
                        .build_branch_service_route_config(
                            &base_environment,
                            base_workload_id,
                            environment.id,
                        )
                        .await
                    {
                        Ok(Some(route)) => {
                            Self::send_branch_service_route_update(
                                &cluster_server,
                                base_environment_id,
                                base_workload_id,
                                environment.id,
                                route,
                            )
                            .await;
                        }
                        Ok(None) => {
                            Self::send_branch_service_route_removal(
                                &cluster_server,
                                base_environment_id,
                                base_workload_id,
                                environment.id,
                            )
                            .await;
                        }
                        Err(err) => {
                            tracing::warn!(
                                base_environment_id = %base_environment_id,
                                branch_environment_id = %environment.id,
                                workload_id = %base_workload_id,
                                error = ?err,
                                "Failed to build branch service route config; falling back to full refresh"
                            );
                            Self::refresh_branch_service_routes_with_logging(
                                &cluster_server,
                                base_environment_id,
                            )
                            .await;
                        }
                    }
                }
                None => {
                    tracing::warn!(
                        environment_id = %environment.id,
                        workload_id = %workload_id,
                        "Branch workload missing base_workload_id; falling back to full refresh"
                    );
                    Self::refresh_branch_service_routes_with_logging(
                        &cluster_server,
                        base_environment_id,
                    )
                    .await;
                }
            }
        }

        self.db
            .update_environment_workload(target_workload_id, containers, persisted_yaml)
            .await
            .map_err(ApiError::from)?;

        Ok(target_workload_id)
    }

    async fn send_branch_service_route_update(
        cluster_server: &KubeClusterServer,
        base_environment_id: Uuid,
        base_workload_id: Uuid,
        branch_environment_id: Uuid,
        route: ProxyBranchRouteConfig,
    ) {
        match cluster_server
            .rpc_client
            .update_branch_service_route(
                tarpc::context::current(),
                base_environment_id,
                base_workload_id,
                route,
            )
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    base_environment_id = %base_environment_id,
                    branch_environment_id = %branch_environment_id,
                    workload_id = %base_workload_id,
                    error = %err,
                    "KubeManager rejected branch service route update"
                );
            }
            Err(err) => {
                tracing::warn!(
                    base_environment_id = %base_environment_id,
                    branch_environment_id = %branch_environment_id,
                    workload_id = %base_workload_id,
                    error = %err,
                    "Failed to send branch service route update to KubeManager"
                );
            }
        }
    }

    async fn send_branch_service_route_removal(
        cluster_server: &KubeClusterServer,
        base_environment_id: Uuid,
        base_workload_id: Uuid,
        branch_environment_id: Uuid,
    ) {
        match cluster_server
            .rpc_client
            .remove_branch_service_route(
                tarpc::context::current(),
                base_environment_id,
                base_workload_id,
                branch_environment_id,
            )
            .await
        {
            Ok(Ok(())) => {}
            Ok(Err(err)) => {
                tracing::warn!(
                    base_environment_id = %base_environment_id,
                    branch_environment_id = %branch_environment_id,
                    workload_id = %base_workload_id,
                    error = %err,
                    "KubeManager rejected branch service route removal"
                );
            }
            Err(err) => {
                tracing::warn!(
                    base_environment_id = %base_environment_id,
                    branch_environment_id = %branch_environment_id,
                    workload_id = %base_workload_id,
                    error = %err,
                    "Failed to send branch service route removal to KubeManager"
                );
            }
        }
    }

    async fn refresh_branch_service_routes_with_logging(
        cluster_server: &KubeClusterServer,
        base_environment_id: Uuid,
    ) {
        if let Err(err) = cluster_server
            .rpc_client
            .refresh_branch_service_routes(tarpc::context::current(), base_environment_id)
            .await
        {
            tracing::warn!(
                base_environment_id = %base_environment_id,
                error = %err,
                "Failed to refresh branch service routes during fallback"
            );
        }
    }

    async fn build_branch_workload_manifest(
        &self,
        environment: &lapdev_db_entities::kube_environment::Model,
        existing_workload: &lapdev_common::kube::KubeEnvironmentWorkload,
        kind: KubeWorkloadKind,
        containers: &[lapdev_common::kube::KubeContainerInfo],
    ) -> Result<(KubeWorkloadWithResources, String, HashMap<String, String>), ApiError> {
        let base_workload_name = existing_workload.name.clone();
        let env_suffix = environment.id.to_string();
        let branch_workload_name = if base_workload_name.ends_with(&env_suffix) {
            base_workload_name.clone()
        } else {
            format!("{}-{}", base_workload_name, environment.id)
        };
        let branch_selector = build_branch_service_selector(&branch_workload_name);

        let (mut workload_with_resources, persisted_yaml) = Self::build_standard_workload_manifest(
            kind,
            &existing_workload.workload_yaml,
            containers,
        )?;

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

            let branch_service_name = format!("{}-{}", service.name, environment.id);
            updated_services.insert(
                branch_service_name.clone(),
                KubeServiceWithYaml {
                    yaml: service.yaml.clone(),
                    details: KubeServiceDetails {
                        name: branch_service_name,
                        ports: service.ports.clone(),
                        selector: branch_selector.clone(),
                    },
                },
            );
        }
        workload_with_resources.services = updated_services;
        workload_with_resources.configmaps.clear();
        workload_with_resources.secrets.clear();

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
        extra_labels.insert(
            "lapdev.base-workload-id".to_string(),
            existing_workload.id.to_string(),
        );

        Ok((workload_with_resources, persisted_yaml, extra_labels))
    }

    fn build_standard_workload_manifest(
        kind: KubeWorkloadKind,
        original_yaml: &str,
        containers: &[lapdev_common::kube::KubeContainerInfo],
    ) -> Result<(KubeWorkloadWithResources, String), ApiError> {
        let rebuilt_yaml =
            rebuild_workload_yaml(&kind, original_yaml, containers, None).map_err(|err| {
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
            KubeWorkloadWithResources {
                workload,
                services: HashMap::new(),
                configmaps: HashMap::new(),
                secrets: HashMap::new(),
            },
            rebuilt_yaml,
        ))
    }

    fn map_workload_to_environment(
        environment: &KubeEnvironment,
        mut workload: KubeEnvironmentWorkload,
    ) -> Result<KubeEnvironmentWorkload, ApiError> {
        if workload.environment_id == environment.id {
            return Ok(workload);
        }

        if let Some(base_environment_id) = environment.base_environment_id {
            if workload.environment_id == base_environment_id {
                if workload.base_workload_id.is_none() {
                    workload.base_workload_id = Some(workload.id);
                }
                workload.environment_id = environment.id;
                return Ok(workload);
            }
        }

        Err(ApiError::InvalidRequest(
            "Workload does not belong to this environment".to_string(),
        ))
    }
}

pub(super) fn rename_workload_yaml(
    workload: &mut KubeWorkloadYamlOnly,
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    match workload {
        KubeWorkloadYamlOnly::Deployment(yaml) => {
            rename_deployment_yaml(yaml, new_name, selector_labels)
        }
        KubeWorkloadYamlOnly::StatefulSet(yaml) => {
            rename_statefulset_yaml(yaml, new_name, selector_labels)
        }
        KubeWorkloadYamlOnly::DaemonSet(yaml) => {
            rename_daemonset_yaml(yaml, new_name, selector_labels)
        }
        KubeWorkloadYamlOnly::ReplicaSet(yaml) => {
            rename_replicaset_yaml(yaml, new_name, selector_labels)
        }
        KubeWorkloadYamlOnly::Pod(yaml) => rename_pod_yaml(yaml, new_name, selector_labels),
        KubeWorkloadYamlOnly::Job(yaml) => rename_job_yaml(yaml, new_name, selector_labels),
        KubeWorkloadYamlOnly::CronJob(yaml) => rename_cronjob_yaml(yaml, new_name, selector_labels),
    }
}

pub(super) fn build_branch_service_selector(workload_name: &str) -> BTreeMap<String, String> {
    let mut selector = BTreeMap::new();
    selector.insert("app".to_string(), workload_name.to_string());
    selector
}

pub(super) fn rename_service_yaml(
    yaml: &str,
    new_service_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<String, ApiError> {
    let mut service: Service = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse service YAML for rename: {}", err))
    })?;

    service.metadata.name = Some(new_service_name.to_string());
    let mut labels = BTreeMap::new();
    labels.insert("app".to_string(), new_service_name.to_string());
    service.metadata.labels = Some(labels);

    if let Some(spec) = service.spec.as_mut() {
        spec.selector = Some(selector_labels.clone());
    }

    serde_yaml::to_string(&service).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed service YAML: {}", err))
    })
}

fn rename_deployment_yaml(
    yaml: &mut String,
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut deployment: Deployment = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse deployment YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut deployment.metadata, new_name, selector_labels);

    if let Some(spec) = deployment.spec.as_mut() {
        update_selector_labels(&mut spec.selector, selector_labels);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, new_name, selector_labels);
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
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut statefulset: StatefulSet = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse statefulset YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut statefulset.metadata, new_name, selector_labels);

    if let Some(spec) = statefulset.spec.as_mut() {
        update_selector_labels(&mut spec.selector, selector_labels);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, new_name, selector_labels);
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
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut daemonset: DaemonSet = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse daemonset YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut daemonset.metadata, new_name, selector_labels);

    if let Some(spec) = daemonset.spec.as_mut() {
        update_selector_labels(&mut spec.selector, selector_labels);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, new_name, selector_labels);
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
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut replicaset: ReplicaSet = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!(
            "Failed to parse replicaset YAML for rename: {}",
            err
        ))
    })?;

    update_object_meta(&mut replicaset.metadata, new_name, selector_labels);

    if let Some(spec) = replicaset.spec.as_mut() {
        update_selector_labels(&mut spec.selector, selector_labels);
        if let Some(template) = spec.template.as_mut() {
            if let Some(metadata) = template.metadata.as_mut() {
                update_object_meta(metadata, new_name, selector_labels);
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

fn rename_pod_yaml(
    yaml: &mut String,
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut pod: Pod = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse pod YAML for rename: {}", err))
    })?;

    update_object_meta(&mut pod.metadata, new_name, selector_labels);

    *yaml = serde_yaml::to_string(&pod).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed pod YAML: {}", err))
    })?;
    Ok(())
}

fn rename_job_yaml(
    yaml: &mut String,
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut job: Job = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse job YAML for rename: {}", err))
    })?;

    update_object_meta(&mut job.metadata, new_name, selector_labels);

    if let Some(spec) = job.spec.as_mut() {
        update_optional_selector_labels(&mut spec.selector, selector_labels);
        if let Some(template) = spec.template.metadata.as_mut() {
            update_object_meta(template, new_name, selector_labels);
        }
    }

    *yaml = serde_yaml::to_string(&job).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed job YAML: {}", err))
    })?;
    Ok(())
}

fn rename_cronjob_yaml(
    yaml: &mut String,
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) -> Result<(), ApiError> {
    let mut cronjob: CronJob = serde_yaml::from_str(yaml).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to parse cronjob YAML for rename: {}", err))
    })?;

    update_object_meta(&mut cronjob.metadata, new_name, selector_labels);

    if let Some(spec) = cronjob.spec.as_mut() {
        let job_template = &mut spec.job_template;
        if let Some(job_spec) = job_template.spec.as_mut() {
            update_optional_selector_labels(&mut job_spec.selector, selector_labels);
            if let Some(template) = job_spec.template.metadata.as_mut() {
                update_object_meta(template, new_name, selector_labels);
            }
        }
    }

    *yaml = serde_yaml::to_string(&cronjob).map_err(|err| {
        ApiError::InvalidRequest(format!("Failed to serialize renamed cronjob YAML: {}", err))
    })?;
    Ok(())
}

fn update_object_meta(
    metadata: &mut ObjectMeta,
    new_name: &str,
    selector_labels: &BTreeMap<String, String>,
) {
    metadata.name = Some(new_name.to_string());
    ensure_labels_from_map(&mut metadata.labels, selector_labels);
}

fn ensure_labels_from_map(
    labels: &mut Option<BTreeMap<String, String>>,
    desired: &BTreeMap<String, String>,
) {
    *labels = Some(desired.clone());
}

fn update_selector_labels(selector: &mut LabelSelector, desired: &BTreeMap<String, String>) {
    selector.match_labels = Some(desired.clone());
}

fn update_optional_selector_labels(
    selector: &mut Option<LabelSelector>,
    desired: &BTreeMap<String, String>,
) {
    match selector {
        Some(existing) => update_selector_labels(existing, desired),
        None => {
            let mut new_selector = LabelSelector::default();
            update_selector_labels(&mut new_selector, desired);
            *selector = Some(new_selector);
        }
    }
}

fn to_btreemap(labels: &HashMap<String, String>) -> BTreeMap<String, String> {
    labels
        .iter()
        .map(|(key, value)| (key.clone(), value.clone()))
        .collect()
}
