use k8s_openapi::api::core::v1::{ConfigMap, Secret};

pub fn clean_configmap(configmap: ConfigMap) -> ConfigMap {
    ConfigMap {
        metadata: clean_metadata(configmap.metadata),
        data: configmap.data,
        binary_data: configmap.binary_data,
        immutable: configmap.immutable,
    }
}

pub fn clean_secret(secret: Secret) -> Secret {
    Secret {
        metadata: clean_metadata(secret.metadata),
        data: secret.data,
        string_data: secret.string_data,
        type_: secret.type_,
        immutable: secret.immutable,
    }
}

fn clean_metadata(
    metadata: k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta,
) -> k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
    k8s_openapi::apimachinery::pkg::apis::meta::v1::ObjectMeta {
        name: metadata.name,
        labels: metadata.labels,
        ..Default::default()
    }
}
