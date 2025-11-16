use super::*;
use chrono::Utc;
use lapdev_common::kube::{
    KubeContainerImage, KubeContainerInfo, KubeContainerPort, KubeServicePort, KubeWorkloadKind,
};
use lapdev_db_entities::kube_app_catalog_workload;
use lapdev_kube_rpc::{ResourceChangeEvent, ResourceChangeType, ResourceType};
use sea_orm::prelude::Json;
use serde_json::json;
use std::collections::{BTreeMap, BTreeSet};
use uuid::Uuid;

#[test]
fn extract_workload_from_yaml_reads_ready_replicas_for_deployments() {
    let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: default
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
  readyReplicas: 3
"#;

    let extracted = extract_workload_from_yaml(ResourceType::Deployment, deployment_yaml).unwrap();
    assert_eq!(extracted.ready_replicas, Some(3));
}

#[test]
fn extract_workload_from_yaml_reads_ready_replicas_for_jobs() {
    let job_yaml = r#"
apiVersion: batch/v1
kind: Job
metadata:
  name: data-migrate
  namespace: default
spec:
  template:
    metadata:
      labels:
        job-name: data-migrate
    spec:
      containers:
        - name: migrate
          image: alpine:3
          command: ["/bin/true"]
      restartPolicy: Never
status:
  succeeded: 1
"#;

    let extracted = extract_workload_from_yaml(ResourceType::Job, job_yaml).unwrap();
    assert_eq!(extracted.ready_replicas, Some(1));
}

#[test]
fn extract_workload_from_yaml_returns_labels_even_without_status() {
    let deployment_yaml = format!(
        r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: web
  namespace: default
  labels:
    lapdev.io/branch-environment-id: "{env_id}"
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
"#,
        env_id = Uuid::new_v4()
    );

    let extracted =
        extract_workload_from_yaml(ResourceType::Deployment, deployment_yaml.as_str()).unwrap();
    assert!(extracted.ready_replicas.is_none());
    assert!(extracted
        .metadata_labels
        .contains_key("lapdev.io/branch-environment-id"));
}

#[test]
fn spec_section_from_yaml_ignores_status_updates() {
    let base_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: default
  resourceVersion: "123"
spec:
  replicas: 2
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      containers:
        - name: api
          image: nginx:1.27
status:
  readyReplicas: 1
"#;

    let status_only_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: default
  resourceVersion: "456"
spec:
  replicas: 2
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      containers:
        - name: api
          image: nginx:1.27
status:
  readyReplicas: 0
"#;

    let base_spec = spec_section_from_yaml(base_yaml)
        .expect("base spec parsing should succeed")
        .expect("base spec should be present");
    let status_spec = spec_section_from_yaml(status_only_yaml)
        .expect("status-only spec parsing should succeed")
        .expect("status-only spec should be present");

    assert_eq!(base_spec, status_spec);
}

#[test]
fn spec_section_from_yaml_ignores_replica_changes() {
    let base_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: default
spec:
  replicas: 2
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      containers:
        - name: api
          image: nginx:1.27
"#;

    let scaled_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: default
spec:
  replicas: 3
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      containers:
        - name: api
          image: nginx:1.27
"#;

    let base_spec = spec_section_from_yaml(base_yaml)
        .expect("base spec parsing should succeed")
        .expect("base spec should be present");
    let scaled_spec = spec_section_from_yaml(scaled_yaml)
        .expect("scaled spec parsing should succeed")
        .expect("scaled spec should be present");

    assert_eq!(base_spec, scaled_spec);
}

#[test]
fn extract_workload_from_yaml_returns_spec_snapshot() {
    let deployment_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: default
spec:
  replicas: 2
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      containers:
        - name: api
          image: nginx:1.27
"#;

    let extracted = extract_workload_from_yaml(ResourceType::Deployment, deployment_yaml)
        .expect("yaml should parse");
    let spec_snapshot = extracted
        .spec_snapshot
        .expect("deployment should expose spec snapshot");

    let spec_from_yaml = spec_section_from_yaml(deployment_yaml)
        .expect("spec extraction should succeed")
        .expect("spec should be present");

    assert_eq!(spec_snapshot, spec_from_yaml);
}

struct WorkloadTestFixture {
    event: ResourceChangeEvent,
    workload: kube_app_catalog_workload::Model,
    base_yaml: String,
    base_containers: Vec<KubeContainerInfo>,
    base_ports: Vec<KubeServicePort>,
    base_spec_snapshot: serde_json::Value,
    base_labels: BTreeMap<String, String>,
    base_configmaps: BTreeSet<String>,
    base_secrets: BTreeSet<String>,
}

impl WorkloadTestFixture {
    fn new() -> Self {
        let base_yaml = r#"
apiVersion: apps/v1
kind: Deployment
metadata:
  name: api
  namespace: default
spec:
  replicas: 2
  selector:
    matchLabels:
      app: api
  template:
    metadata:
      labels:
        app: api
    spec:
      containers:
        - name: api
          image: nginx:1.27
          resources:
            requests:
              cpu: 100m
              memory: 128Mi
            limits:
              cpu: 200m
              memory: 256Mi
"#
        .trim()
        .to_string();

        let base_spec_snapshot = spec_section_from_yaml(&base_yaml)
            .expect("spec snapshot extraction should succeed")
            .expect("spec snapshot should be present");

        let base_containers = vec![KubeContainerInfo {
            name: "api".to_string(),
            original_image: "nginx:1.27".to_string(),
            image: KubeContainerImage::FollowOriginal,
            cpu_request: Some("100m".to_string()),
            cpu_limit: Some("200m".to_string()),
            memory_request: Some("128Mi".to_string()),
            memory_limit: Some("256Mi".to_string()),
            env_vars: Vec::new(),
            original_env_vars: Vec::new(),
            ports: vec![KubeContainerPort {
                name: Some("http".to_string()),
                container_port: 8080,
                protocol: Some("TCP".to_string()),
            }],
        }];

        let base_ports = vec![KubeServicePort {
            name: Some("http".to_string()),
            port: 80,
            target_port: Some(8080),
            protocol: Some("TCP".to_string()),
            node_port: None,
            original_target_port: Some(8080),
        }];

        let mut base_configmaps = BTreeSet::new();
        base_configmaps.insert("cfg-main".to_string());
        let mut base_secrets = BTreeSet::new();
        base_secrets.insert("sec-main".to_string());

        let base_labels = BTreeMap::from([("app".to_string(), "api".to_string())]);

        let workload = kube_app_catalog_workload::Model {
            id: Uuid::new_v4(),
            created_at: Utc::now().into(),
            deleted_at: None,
            app_catalog_id: Uuid::new_v4(),
            cluster_id: Uuid::new_v4(),
            name: "api".to_string(),
            namespace: "default".to_string(),
            kind: KubeWorkloadKind::Deployment.to_string(),
            containers: Json::from(serde_json::to_value(&base_containers).unwrap()),
            ports: Json::from(serde_json::to_value(&base_ports).unwrap()),
            workload_yaml: base_yaml.clone(),
            catalog_sync_version: 1,
        };

        let event = ResourceChangeEvent {
            namespace: "default".to_string(),
            resource_type: ResourceType::Deployment,
            resource_name: "api".to_string(),
            change_type: ResourceChangeType::Updated,
            resource_version: "1".to_string(),
            resource_yaml: Some(base_yaml.clone()),
            timestamp: Utc::now(),
        };

        Self {
            event,
            workload,
            base_yaml,
            base_containers,
            base_ports,
            base_spec_snapshot,
            base_labels,
            base_configmaps,
            base_secrets,
        }
    }
}

fn compute_update(
    event: &ResourceChangeEvent,
    workload: &kube_app_catalog_workload::Model,
    new_containers: &[KubeContainerInfo],
    service_ports: &[KubeServicePort],
    yaml: &str,
    new_spec_snapshot: Option<&serde_json::Value>,
    new_labels: &BTreeMap<String, String>,
    existing_labels: Option<&BTreeMap<String, String>>,
    new_configmaps: &BTreeSet<String>,
    new_secrets: &BTreeSet<String>,
    existing_dependencies: Option<&(BTreeSet<String>, BTreeSet<String>)>,
) -> CatalogWorkloadUpdate {
    KubeClusterServer::compute_catalog_workload_update(
        event,
        workload,
        new_containers,
        service_ports,
        yaml,
        new_spec_snapshot,
        new_labels,
        existing_labels,
        new_configmaps,
        new_secrets,
        existing_dependencies,
    )
    .expect("catalog workload update computation should succeed")
}

#[test]
fn compute_catalog_workload_update_detects_no_changes() {
    let fixture = WorkloadTestFixture::new();
    let new_containers = fixture.base_containers.clone();
    let service_ports = fixture.base_ports.clone();
    let new_labels = fixture.base_labels.clone();
    let existing_labels = fixture.base_labels.clone();
    let new_configmaps = fixture.base_configmaps.clone();
    let new_secrets = fixture.base_secrets.clone();
    let existing_dependencies = (
        fixture.base_configmaps.clone(),
        fixture.base_secrets.clone(),
    );
    let spec_snapshot = fixture.base_spec_snapshot.clone();

    let update = compute_update(
        &fixture.event,
        &fixture.workload,
        &new_containers,
        &service_ports,
        &fixture.base_yaml,
        Some(&spec_snapshot),
        &new_labels,
        Some(&existing_labels),
        &new_configmaps,
        &new_secrets,
        Some(&existing_dependencies),
    );

    assert!(!update.requires_catalog_bump());
    assert!(!update.containers_changed);
    assert!(!update.ports_changed);
    assert!(!update.spec_changed);
    assert!(!update.labels_changed);
    assert!(!update.dependencies_changed);
    assert!(!update.has_model_updates());
}

#[test]
fn compute_catalog_workload_update_detects_spec_change() {
    let fixture = WorkloadTestFixture::new();
    let new_containers = fixture.base_containers.clone();
    let service_ports = fixture.base_ports.clone();
    let new_labels = fixture.base_labels.clone();
    let existing_labels = fixture.base_labels.clone();
    let new_configmaps = fixture.base_configmaps.clone();
    let new_secrets = fixture.base_secrets.clone();
    let existing_dependencies = (
        fixture.base_configmaps.clone(),
        fixture.base_secrets.clone(),
    );

    let mut spec_snapshot = fixture.base_spec_snapshot.clone();
    spec_snapshot["revisionHistoryLimit"] = json!(5);

    let new_yaml = fixture
        .base_yaml
        .replace("replicas: 2", "replicas: 2\n  revisionHistoryLimit: 5")
        .to_string();

    let update = compute_update(
        &fixture.event,
        &fixture.workload,
        &new_containers,
        &service_ports,
        &new_yaml,
        Some(&spec_snapshot),
        &new_labels,
        Some(&existing_labels),
        &new_configmaps,
        &new_secrets,
        Some(&existing_dependencies),
    );

    assert!(update.requires_catalog_bump());
    assert!(update.spec_changed);
    assert!(update.workload_yaml.is_some());
    assert!(!update.containers_changed);
    assert!(!update.ports_changed);
    assert!(!update.labels_changed);
    assert!(!update.dependencies_changed);
}

#[test]
fn compute_catalog_workload_update_detects_container_change() {
    let fixture = WorkloadTestFixture::new();
    let mut new_containers = fixture.base_containers.clone();
    new_containers[0].cpu_request = Some("250m".to_string());
    new_containers[0].cpu_limit = Some("500m".to_string());

    let service_ports = fixture.base_ports.clone();
    let new_labels = fixture.base_labels.clone();
    let existing_labels = fixture.base_labels.clone();
    let new_configmaps = fixture.base_configmaps.clone();
    let new_secrets = fixture.base_secrets.clone();
    let existing_dependencies = (
        fixture.base_configmaps.clone(),
        fixture.base_secrets.clone(),
    );
    let spec_snapshot = fixture.base_spec_snapshot.clone();

    let update = compute_update(
        &fixture.event,
        &fixture.workload,
        &new_containers,
        &service_ports,
        &fixture.base_yaml,
        Some(&spec_snapshot),
        &new_labels,
        Some(&existing_labels),
        &new_configmaps,
        &new_secrets,
        Some(&existing_dependencies),
    );

    assert!(update.requires_catalog_bump());
    assert!(update.containers_changed);
    assert!(update.containers.is_some());
    assert!(!update.ports_changed);
    assert!(!update.spec_changed);
    assert!(!update.labels_changed);
    assert!(!update.dependencies_changed);
}

#[test]
fn compute_catalog_workload_update_detects_port_change() {
    let fixture = WorkloadTestFixture::new();
    let new_containers = fixture.base_containers.clone();
    let mut service_ports = fixture.base_ports.clone();
    service_ports[0].port = 443;
    service_ports[0].target_port = Some(8443);
    service_ports[0].original_target_port = Some(8443);

    let new_labels = fixture.base_labels.clone();
    let existing_labels = fixture.base_labels.clone();
    let new_configmaps = fixture.base_configmaps.clone();
    let new_secrets = fixture.base_secrets.clone();
    let existing_dependencies = (
        fixture.base_configmaps.clone(),
        fixture.base_secrets.clone(),
    );
    let spec_snapshot = fixture.base_spec_snapshot.clone();

    let update = compute_update(
        &fixture.event,
        &fixture.workload,
        &new_containers,
        &service_ports,
        &fixture.base_yaml,
        Some(&spec_snapshot),
        &new_labels,
        Some(&existing_labels),
        &new_configmaps,
        &new_secrets,
        Some(&existing_dependencies),
    );

    assert!(update.requires_catalog_bump());
    assert!(update.ports_changed);
    assert!(update.ports.is_some());
    assert!(!update.containers_changed);
    assert!(!update.spec_changed);
    assert!(!update.labels_changed);
    assert!(!update.dependencies_changed);
}

#[test]
fn compute_catalog_workload_update_detects_label_change() {
    let fixture = WorkloadTestFixture::new();
    let new_containers = fixture.base_containers.clone();
    let service_ports = fixture.base_ports.clone();
    let mut new_labels = fixture.base_labels.clone();
    new_labels.insert("version".to_string(), "v2".to_string());
    let existing_labels = fixture.base_labels.clone();
    let new_configmaps = fixture.base_configmaps.clone();
    let new_secrets = fixture.base_secrets.clone();
    let existing_dependencies = (
        fixture.base_configmaps.clone(),
        fixture.base_secrets.clone(),
    );
    let spec_snapshot = fixture.base_spec_snapshot.clone();

    let update = compute_update(
        &fixture.event,
        &fixture.workload,
        &new_containers,
        &service_ports,
        &fixture.base_yaml,
        Some(&spec_snapshot),
        &new_labels,
        Some(&existing_labels),
        &new_configmaps,
        &new_secrets,
        Some(&existing_dependencies),
    );

    assert!(update.requires_catalog_bump());
    assert!(update.labels_changed);
    assert!(!update.containers_changed);
    assert!(!update.ports_changed);
    assert!(!update.spec_changed);
    assert!(!update.dependencies_changed);
    assert!(!update.has_model_updates());
}

#[test]
fn compute_catalog_workload_update_detects_dependency_change() {
    let fixture = WorkloadTestFixture::new();
    let new_containers = fixture.base_containers.clone();
    let service_ports = fixture.base_ports.clone();
    let new_labels = fixture.base_labels.clone();
    let existing_labels = fixture.base_labels.clone();
    let mut new_configmaps = fixture.base_configmaps.clone();
    new_configmaps.insert("cfg-extra".to_string());
    let new_secrets = fixture.base_secrets.clone();
    let existing_dependencies = (
        fixture.base_configmaps.clone(),
        fixture.base_secrets.clone(),
    );
    let spec_snapshot = fixture.base_spec_snapshot.clone();

    let update = compute_update(
        &fixture.event,
        &fixture.workload,
        &new_containers,
        &service_ports,
        &fixture.base_yaml,
        Some(&spec_snapshot),
        &new_labels,
        Some(&existing_labels),
        &new_configmaps,
        &new_secrets,
        Some(&existing_dependencies),
    );

    assert!(update.requires_catalog_bump());
    assert!(update.dependencies_changed);
    assert!(!update.containers_changed);
    assert!(!update.ports_changed);
    assert!(!update.spec_changed);
    assert!(!update.labels_changed);
}
